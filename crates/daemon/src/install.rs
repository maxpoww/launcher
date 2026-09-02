//! The install flow: drag-to-install pending tiles, resolving finished
//! installs to their real apps, uninstall bookkeeping, the "try it"
//! launch, and the nix worker's event handling.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use calloop::timer::{TimeoutAction, Timer};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};
use waverunner_core::index::AppEntry;

use waverunner_proto::Command;

use crate::state::Target;
use crate::{apps, content, launch, nix};
use crate::{App, PkgIndexState};

/// A package dropped into the Apps grid and now installing in place. It
/// renders as a grid tile (dimmed, "Installing…") at the slot it was
/// dropped on, and — once the switch succeeds — is replaced there by the
/// real app. A failure keeps the tile (flashing "Failed"); clicking it
/// retries.
pub(crate) struct PendingInstall {
    /// nixpkgs attribute — the install target and the tile's grid id.
    pub(crate) attr: String,
    /// Display name / version for the tile label.
    pub(crate) name: String,
    pub(crate) version: String,
    /// Desktop ids the package ships: when one appears as a real app
    /// after install, that app takes this tile's slot.
    pub(crate) desktop_ids: Vec<String>,
    /// Grid id the installed app should land *before* (`None` = end) —
    /// the app displayed at the drop slot when it was let go.
    pub(crate) anchor: Option<String>,
    /// Storage page of a grid drop with no anchor (a page's empty tail,
    /// or the ghost page): the app lands at THIS page's end. Without it,
    /// anchorless installs fell to the order's default — the LAST page —
    /// teleporting an end-of-page-1 drop onto page 2 (Max, 2026-08-31).
    pub(crate) grid_page: Option<usize>,
    /// Dropped on the dock instead of the grid: the tile pins there at this
    /// slot (not a grid cell), and the finished app takes the same slot.
    pub(crate) dock_slot: Option<usize>,
    /// Dropped into (or onto, creating) a box: the tile is a member of this
    /// group id while installing, and the finished app replaces it in place.
    pub(crate) box_dest: Option<String>,
    /// Reserved-tail icon slot (`0..PENDING_INSTALL_CAP`); its texture
    /// layer is `pkg_layer_base + RANK_HITS_MAX + slot`.
    pub(crate) icon_slot: usize,
    /// Snapshot of the package's rasterized icon, re-uploaded after every
    /// rescan (which rebuilds the texture array). Empty = letter tile.
    pub(crate) icon_pixels: Vec<u8>,
    /// True when the icon is a generated letter tile, not a real icon.
    pub(crate) placeholder: bool,
    /// Last attempt failed; the tile shows "Failed" and a click retries.
    pub(crate) failed: bool,
    /// When the (current) install attempt began — drives the progress ring.
    /// Reset on a retry so the ring restarts from empty.
    pub(crate) started: Instant,
    /// Set when the rebuild actually finished (`Done` ok): the ring then
    /// eases to a full circle over [`INSTALL_RING_FILL`] before the deferred
    /// rescan swaps in the real app, so even a quick install fills the ring
    /// instead of popping at 20%.
    pub(crate) completed_at: Option<Instant>,
    /// Whether the post-fill rescan has been fired (so it fires just once).
    pub(crate) rescan_fired: bool,
}

/// On-disk snapshot of one [`PendingInstall`] tile (`pending-installs.json`):
/// everything needed to re-arm the install after a daemon restart — an HM
/// switch restarts changed user units mid-session, but the root helper's
/// rebuild continues regardless, so on restart the re-armed
/// [`crate::applier::apply_install`] fast-completes (a run landed while we
/// were dead), joins the still-building run, or retries a failed one.
/// The tile's icon pixels live in a sidecar `pending-icon-<attr>.rgba`.
#[derive(Serialize, Deserialize)]
struct SavedPending {
    attr: String,
    name: String,
    version: String,
    desktop_ids: Vec<String>,
    anchor: Option<String>,
    grid_page: Option<usize>,
    dock_slot: Option<usize>,
    box_dest: Option<String>,
    /// Unix epoch the (current) attempt began — restored so the progress
    /// ring resumes where it was instead of restarting from zero.
    started_epoch: f64,
    failed: bool,
    placeholder: bool,
}

/// Everything install-related that must survive a daemon restart: the
/// pending grid tiles and the tile-less managed installs (dock drops /
/// slots-full fallback).
#[derive(Serialize, Deserialize, Default)]
struct SavedInstallState {
    tiles: Vec<SavedPending>,
    managed: Vec<String>,
}

/// `pending-installs.json` in the daemon's data dir.
fn pending_state_path() -> PathBuf {
    crate::persist::data_path("pending-installs.json")
}

/// Sidecar holding one pending tile's raw RGBA icon pixels. The attr
/// charset (`[A-Za-z0-9._-]`, see [`crate::applier`]) is filename-safe.
fn pending_icon_path(attr: &str) -> PathBuf {
    crate::persist::data_path(&format!("pending-icon-{attr}.rgba"))
}

/// Current unix epoch as fractional seconds.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Progress fraction for an install's ring, given how long it has been
/// running. `nixos-rebuild` reports no real percentage, so this fakes the
/// feel of a real installer with two eased phases: a brisk climb to ~85%,
/// then a slow crawl through the last stretch that never quite fills. The
/// tile is replaced by the real app on completion, which reads as 100%.
pub(crate) fn install_ring_progress(elapsed: Duration) -> f32 {
    // `nixos-rebuild` reports no real percentage, and any exponential ease is
    // front-loaded (rushes early, crawls forever near the top). So advance
    // *linearly* over an expected build window — an even pace, where the first
    // half takes as long as the second — and only if the build overruns that
    // estimate do we ease slowly through the last sliver. The tile is replaced
    // by the real app on completion, which reads as 100% however far we got.
    const EXPECTED: f32 = 55.0; // seconds of even, linear climb to `LINEAR_TO`
    const LINEAR_TO: f32 = 0.85;
    let t = elapsed.as_secs_f32();
    if t < EXPECTED {
        LINEAR_TO * (t / EXPECTED)
    } else {
        // Past the estimate: crawl the remaining sliver so it still inches.
        LINEAR_TO + 0.12 * (1.0 - (-(t - EXPECTED) / 40.0).exp())
    }
}

/// How long the ring takes to ease from wherever it was to a full circle
/// once the rebuild finishes.
pub(crate) const INSTALL_RING_FILL: Duration = Duration::from_millis(420);

/// After the ring fills, a glass-shine streak sweeps across the icon for this
/// long before the real app is swapped in — a slow, deliberate reveal.
pub(crate) const INSTALL_SHINE: Duration = Duration::from_millis(950);

/// Total time a finished install is held before the real app swaps in: the
/// ring fill, then the shine sweep. Computed from the two so they never drift.
/// The app's appearance is deferred this long so the whole completion flourish
/// always plays.
pub(crate) const INSTALL_HOLD: Duration = INSTALL_RING_FILL.saturating_add(INSTALL_SHINE);

/// Shine-sweep progress in `0.0..=1.0` for a finished install, or a negative
/// value while the ring is still filling (no shine yet) or when not finished.
pub(crate) fn install_shine(completed_at: Option<Instant>) -> f32 {
    match completed_at {
        Some(done) => {
            let e = done.elapsed();
            if e < INSTALL_RING_FILL {
                -1.0
            } else {
                ((e - INSTALL_RING_FILL).as_secs_f32() / INSTALL_SHINE.as_secs_f32())
                    .clamp(0.0, 1.0)
            }
        }
        None => -1.0,
    }
}

/// A just-installed app surfaced on the dock because the launcher was closed
/// when it finished: a one-shot shine plays, and it stays there — temp-pinned
/// and hidden from the grid — until it has been opened and then closed, when
/// it returns to its grid slot. In-memory only; never persisted.
pub(crate) struct InstallNotify {
    /// The installed app's grid entry id (temp-pinned onto the dock).
    pub(crate) id: String,
    /// When the one-shot dock shine started.
    pub(crate) shine_at: Instant,
    /// Whether the app has been opened at least once (a window seen): only
    /// then does its next close return it to the grid.
    pub(crate) seen_running: bool,
}

/// One-shot dock-shine progress `0.0..=1.0` for a just-installed notify, or a
/// negative value once the single sweep has played out.
pub(crate) fn dock_notify_shine(shine_at: Instant) -> f32 {
    let e = shine_at.elapsed();
    if e >= INSTALL_SHINE {
        -1.0
    } else {
        (e.as_secs_f32() / INSTALL_SHINE.as_secs_f32()).clamp(0.0, 1.0)
    }
}

/// A webapp installs instantly (it's only a classification flip), but we fake
/// this much of a "build" so it lands with the exact same progress ring as a
/// package instead of just snapping onto the grid. `+ INSTALL_RING_FILL` of
/// completion sweep lands the whole thing at ~4 s.
pub(crate) const WEBAPP_BUILD: Duration = Duration::from_millis(3600);

/// A webapp whose install ring is playing: it is already placed on the grid
/// (and in `managed_webapps`), this just drives its ring for the fake build.
pub(crate) struct PendingWebapp {
    /// The grid entry id (`webapp-<slug>`) the ring draws over.
    pub(crate) id: String,
    pub(crate) started: Instant,
    /// Set once the fake build elapses: the ring then eases to full.
    pub(crate) completed_at: Option<Instant>,
}

impl PendingWebapp {
    /// Ring fraction: a linear climb to `LINEAR_TO` over [`WEBAPP_BUILD`],
    /// then an ease-out to a full ring over [`INSTALL_RING_FILL`] — the same
    /// shape a package shows, but on a known 4 s timeline.
    pub(crate) fn ring_fraction(&self) -> f32 {
        const LINEAR_TO: f32 = 0.85;
        match self.completed_at {
            None => {
                let k =
                    (self.started.elapsed().as_secs_f32() / WEBAPP_BUILD.as_secs_f32()).min(1.0);
                LINEAR_TO * k
            }
            Some(done) => {
                let k = (done.elapsed().as_secs_f32() / INSTALL_RING_FILL.as_secs_f32())
                    .clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - k) * (1.0 - k);
                LINEAR_TO + (1.0 - LINEAR_TO) * ease
            }
        }
    }
}

impl PendingInstall {
    /// The progress-ring fraction to draw right now: the linear time estimate
    /// while building, then — once the rebuild has actually finished — an
    /// ease-out from that point to a full ring over [`INSTALL_RING_FILL`].
    pub(crate) fn ring_fraction(&self) -> f32 {
        match self.completed_at {
            None => install_ring_progress(self.started.elapsed()),
            Some(done) => {
                let base = install_ring_progress(done.duration_since(self.started));
                let k = (done.elapsed().as_secs_f32() / INSTALL_RING_FILL.as_secs_f32())
                    .clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - k) * (1.0 - k); // ease-out to full
                base + (1.0 - base) * ease
            }
        }
    }
}

/// Minimum query length before the package index is ranked — one
/// character matches half of nixpkgs and helps no one.
pub(crate) const PKG_QUERY_MIN: usize = 2;

/// How long a failed install/remove flashes "Failed" on its cell.
pub(crate) const FAIL_FLASH: Duration = Duration::from_secs(5);

/// How long the dock flashes into view when a just-installed app lands
/// (while auto-hidden): the landing bounce plays, then the dock rests 2s
/// so the arrival registers, before it slides back away.
/// (= `BOUNCE_DURATION` + 2s.)
pub(crate) const INSTALL_REVEAL: Duration = Duration::from_millis(550 + 2000);

/// Lowercase alphanumerics only — used to loosely relate a package attr
/// to the desktop id it installs (`chromium` ~ `chromium-browser`).
fn normalize_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl App {
    /// A message from the nix threads: the index became searchable, a
    /// rank answer arrived, or a profile mutation finished.
    pub(crate) fn on_nix_event(&mut self, event: nix::Event) {
        match event {
            nix::Event::IndexReady(count) => {
                info!("package index ready: {count} packages");
                self.pkg_state = PkgIndexState::Ready;
                self.refilter();
            }
            nix::Event::IndexFailed => {
                self.pkg_state = PkgIndexState::Failed;
                self.schedule_frame();
            }
            nix::Event::Ranked { query, hits } => {
                self.pkg_hits = hits;
                // Icons for the previous hits don't fit these; show the
                // generic tile until this query's HitIcons arrive.
                self.pkg_hit_icons.clear();
                self.pkg_hit_placeholders.clear();
                // Re-render only if the answer matches what's typed; an
                // outdated one keeps waiting for its follower.
                let current = query == self.search.query;
                self.pkg_hits_query = Some(query);
                if current {
                    self.refilter();
                }
            }
            nix::Event::HitIcons {
                query,
                icons,
                placeholders,
            } => {
                if Some(&query) == self.pkg_hits_query.as_ref() {
                    self.pkg_hit_icons = icons;
                    self.pkg_hit_placeholders = placeholders;
                    self.upload_pkg_icons();
                    if query == self.search.query {
                        // Transients hold per-entry layer/placeholder:
                        // rebuild them onto the fresh icons.
                        self.refilter();
                    }
                }
            }
            nix::Event::Realized {
                attr,
                ok,
                terminal,
                program,
                version,
            } => {
                // A "try it" build finished: run it now that it's built
                // (instant), or flash "Failed" if the build died.
                debug!("realize {attr}: ok={ok} terminal={terminal} program={program:?}");
                self.busy_ids.remove(&attr);
                self.launching.remove(&attr);
                if ok {
                    let exec = if terminal {
                        // Drop into a `nix shell` with the tool on PATH so
                        // it can be run repeatedly, with any arguments,
                        // behind the shared Golem banner naming the
                        // command to run. (A bare `nix run` would run it
                        // once into a shell where it isn't on PATH.)
                        let pkg = attr.rsplit('.').next().unwrap_or(&attr);
                        let prog = program.as_deref().unwrap_or(pkg);
                        let banner = launch::banner_cmd(pkg, version.as_deref(), prog);
                        format!(
                            "export NIXPKGS_ALLOW_UNFREE=1; {banner} \
                             exec nix shell --impure nixpkgs#{attr}"
                        )
                    } else {
                        // A GUI app runs headless and shows its window.
                        format!("NIXPKGS_ALLOW_UNFREE=1 nix run --impure nixpkgs#{attr}")
                    };
                    if let Err(e) = launch::launch(&exec, terminal, &self.config.launch.terminal) {
                        error!("try-launch of {attr} failed: {e:#}");
                    }
                } else {
                    self.flash_failed(attr);
                }
                self.update_hover();
                self.schedule_frame();
            }
            // A startup-reconcile run (F13), not a user install: on success
            // the system now provably matches the list — rescan so any app
            // the run (re)materialized appears. No tile, no busy id.
            nix::Event::Done { id, ok, .. } if id == nix::RECONCILE_ID => {
                if ok {
                    info!("startup reconcile applied; rescanning");
                    self.indexer.request_rescan_fresh();
                } else {
                    warn!("startup reconcile failed; leaving state for the next run");
                }
            }
            nix::Event::Done {
                id,
                ok,
                desktop_ids,
            } => {
                debug!("declarative op for {id}: ok={ok}");
                self.busy_ids.remove(&id);
                let _ = &desktop_ids; // reserved for a future authoritative gui probe
                if let Some(attr) = self.uninstalling.get(&id).cloned() {
                    // An uninstall finished. Only now — on success — drop the
                    // cache entry and dock pin; on failure the package is
                    // still installed and still tracked (apply_uninstall
                    // re-added the list line), so leave both in place.
                    if ok {
                        self.managed.remove(&attr);
                        self.pins.unpin(&id);
                        self.recompute_removable();
                        self.indexer.request_rescan_fresh();
                        // Keep `id` in `uninstalling` so the app stays hidden
                        // (see `is_removing`) through the seconds between the
                        // rebuild finishing and the reindex — `on_apps_loaded`
                        // prunes it once the real entry is actually gone.
                        // Otherwise it flashes back onto the grid mid-rebuild.
                    } else {
                        // Rebuild failed: the app is still installed, so
                        // un-hide it and flash the failure.
                        self.uninstalling.remove(&id);
                        self.flash_failed(id);
                        self.refilter();
                    }
                } else if ok {
                    // Install succeeded: mark THIS attr confirmed and persist.
                    // Only confirmed packages reach managed.json, so other
                    // stages still queued behind it (a batch of simultaneous
                    // drags) don't leak in and get a phantom terminal tile
                    // before their own rebuild lands.
                    if self.managed.contains(&id) {
                        self.managed.confirm(&id);
                        self.recompute_removable();
                    }
                    // A drag-to-install tile: mark it finished so its ring
                    // eases to full, and defer the rescan (see `draw`) until
                    // that fill plays — so a quick install still fills the ring
                    // rather than popping the app in at 20%. No tile (some
                    // other install path) → rescan straight away as before.
                    // Fresh variant clears stale icon-not-found cache entries
                    // so the newly installed app's icon loads.
                    if let Some(p) = self.pending_installs.iter_mut().find(|p| p.attr == id) {
                        p.completed_at = Some(std::time::Instant::now());
                        self.schedule_frame();
                    } else {
                        self.indexer.request_rescan_fresh();
                    }
                } else {
                    // Install failed: roll back the in-memory stage. Nothing
                    // was written to disk, so it stays clean.
                    self.managed.revert(&id);
                    self.managed_install_attrs.retain(|a| a != &id);
                    self.recompute_removable();
                    if let Some(p) = self.pending_installs.iter_mut().find(|p| p.attr == id) {
                        // Drag-to-install tile stays as a persistent "Failed"
                        // cell (click retries, drag-off dismisses).
                        p.failed = true;
                    } else {
                        self.flash_failed(id);
                    }
                }
                self.save_install_state();
                self.update_hover();
                self.schedule_frame();
            }
        }
    }

    /// Recompute which indexed apps are removable: an app is removable
    /// when its id maps to an attr in the waverunner-managed list (i.e.
    /// the launcher installed it — base/system apps are never removable).
    /// Run when either the apps or the managed list change; also
    /// refreshes the set of installed desktop ids used to hide installed
    /// packages.
    pub(crate) fn recompute_removable(&mut self) {
        self.installed_app_ids = self
            .entries
            .iter()
            .zip(&self.kinds)
            .take(self.base_len)
            .filter(|(_, &k)| k == apps::EntryKind::App)
            .map(|(e, _)| e.id.clone())
            .collect();
        let managed = self.managed.removable_ids();
        self.removable_ids = self
            .entries
            .iter()
            .zip(&self.kinds)
            .take(self.base_len)
            .filter(|(_, &k)| k == apps::EntryKind::App)
            .filter(|(e, _)| managed.contains(&e.id))
            .map(|(e, _)| e.id.clone())
            .collect();
    }

    /// Whether a nixpkgs package is already installed on the system —
    /// its attr is in the managed list, or an installed app already
    /// ships one of its `.desktop` ids (base/system apps). Such packages
    /// are hidden from the Install list (you can't install what's already
    /// there).
    pub(crate) fn pkg_installed(&self, p: &nix::PkgEntry) -> bool {
        self.managed.contains(&p.attr)
            || p.icons.iter().any(|s| self.installed_app_ids.contains(s))
            || self.installed_app_ids.contains(&p.attr)
    }

    /// Copy the current package-hit icons into the icon texture array's
    /// reserved tail (base = app icon count of the last `set_icons`).
    pub(crate) fn upload_pkg_icons(&mut self) {
        let base = self.pkg_layer_base;
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for (i, pixels) in self.pkg_hit_icons.iter().enumerate() {
            renderer.update_icon_layer(base + i as u32, pixels);
        }
    }

    /// Append one transient Install-section entry for the `hit_idx`-th
    /// ranked package (kind Package, its own icon from the reserved
    /// texture tail) and return its index. The entry id is the package
    /// attr — the install handle.
    pub(crate) fn push_transient_pkg(&mut self, pkg: &nix::PkgEntry, hit_idx: usize) -> usize {
        let (id, name, version) = (pkg.attr.clone(), pkg.name.clone(), pkg.version.clone());
        let (layer, placeholder) = if hit_idx < self.pkg_hit_icons.len() {
            (
                self.pkg_layer_base + hit_idx as u32,
                self.pkg_hit_placeholders[hit_idx],
            )
        } else {
            // No rasterized icon delivered (shouldn't happen): generic
            // package icon.
            self.asset("asset-pkg").unwrap_or((0, true))
        };
        let entry = AppEntry {
            id,
            name,
            description: Some(version),
            exec: String::new(),
            icon: None,
            startup_wm_class: None,
            needs_terminal: false,
            path: None,
        };
        self.push_transient(entry, apps::EntryKind::Package, placeholder, layer)
    }

    /// Rank the package index against the query and append the top
    /// matches as transient entries, returning their indices for the
    /// Install section.
    pub(crate) fn pkg_results(&mut self) -> Vec<usize> {
        // Lazily load the index the first time the launcher card is actually
        // opened (not during the hidden-state refilters that run at startup and
        // on every rescan). Until this fires the nix thread is parked holding no
        // index memory; one Rank kicks the ~8.6 MB parse (and icon sweep), after
        // which `IndexReady` flips the state to `Ready` and ranking takes over.
        if self.pkg_state == PkgIndexState::Loading
            && !self.pkg_load_kicked
            && self.ui.target() == Target::Open
        {
            self.pkg_load_kicked = true;
            self.nix.request(nix::Request::Rank {
                query: self.search.query.clone(),
            });
        }
        if self.pkg_state != PkgIndexState::Ready {
            return Vec::new();
        }
        // Ranking the index is too slow for the render path: the nix
        // thread does it and answers with a Ranked event, which
        // refilters again. Until then the previous hits keep showing.
        // The empty query is the recommendations storefront; a 1-char
        // query keeps showing whatever is up (no flash to empty).
        let query = self.search.query.clone();
        let rankable = query.is_empty() || query.chars().count() >= PKG_QUERY_MIN;
        if rankable && self.pkg_hits_query.as_deref() != Some(query.as_str()) {
            self.nix.request(nix::Request::Rank { query });
        }
        let hits = self.pkg_hits.clone();
        // Already-installed packages drop out of the list; the original
        // hit index is kept so each shown package keeps its own icon.
        let shown: Vec<(usize, nix::PkgEntry)> = hits
            .into_iter()
            .enumerate()
            .filter(|(_, p)| !self.pkg_installed(p))
            .collect();
        shown
            .into_iter()
            .map(|(i, p)| self.push_transient_pkg(&p, i))
            .collect()
    }

    /// Begin an ephemeral "try it" launch: realize the package
    /// (`nix build`, shown as "Launching…"), then — once it lands (see the
    /// `Realized` handler) — run it without installing. Whether it runs in
    /// a terminal (TUI/CLI) or headless (GUI) is decided from the built
    /// package, not guessed here.
    pub(crate) fn start_launch(&mut self, attr: &str) {
        if self.busy_ids.contains(attr) || self.launching.contains(attr) {
            return;
        }
        info!("try-launching {attr}");
        self.busy_ids.insert(attr.to_owned());
        self.launching.insert(attr.to_owned());
        self.nix.request(nix::Request::Realize {
            attr: attr.to_owned(),
        });
        self.refilter();
    }

    /// Flash "Failed" on `id`'s cell for [`FAIL_FLASH`], then clear it.
    pub(crate) fn flash_failed(&mut self, id: String) {
        self.failed_ids.insert(id, Instant::now());
        let timer = Timer::from_duration(FAIL_FLASH);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.failed_ids.retain(|_, t| t.elapsed() < FAIL_FLASH);
                app.schedule_frame();
                TimeoutAction::Drop
            })
        {
            warn!("cannot arm fail-flash timer: {e}");
        }
    }

    /// Record `attr` in the managed home-manager list (with the desktop
    /// ids it ships, for later uninstall mapping) and fire the switch
    /// that installs it — no grid tile. Used for the dock-drop path and
    /// the pending-tile slots-full fallback.
    pub(crate) fn start_managed_install(&mut self, attr: &str, desktop_ids: Vec<String>) {
        if self.busy_ids.contains(attr) {
            return;
        }
        // Stage in memory only — packages.nix is NOT written until the
        // install succeeds (Done { ok: true } → managed.confirm()).
        self.managed.stage(attr, desktop_ids.clone());
        self.recompute_removable();
        self.busy_ids.insert(attr.to_owned());
        // Track for dock-pin resolution on success (no pending grid tile).
        self.managed_install_attrs.push(attr.to_owned());
        self.nix.request(nix::Request::Install {
            id: attr.to_owned(),
            attr: attr.to_owned(),
        });
        self.save_install_state();
    }

    /// Begin a drag-to-install: reserve a grid tile at the drop slot for
    /// `attr`, upload its icon into the pending-install texture block,
    /// record the package in the managed list, and fire the switch. The
    /// tile shows "Installing…" until the rescan swaps in the real app
    /// (see [`Self::resolve_pending_installs`]), or "Failed" (retry on
    /// click) if the switch fails.
    ///
    /// `dock_slot` is set when the drop landed on the dock band instead of
    /// the grid: the tile then pins onto the dock at that slot (and the
    /// finished app takes the same slot) rather than riding a grid cell.
    /// `box_dest` is set when the drop landed in/onto a box: the tile is
    /// already a member of that group and the finished app replaces it there.
    pub(crate) fn start_pending_install(
        &mut self,
        attr: &str,
        name: String,
        version: String,
        dock_slot: Option<usize>,
        box_dest: Option<String>,
    ) {
        if self.busy_ids.contains(attr) || self.pending_installs.iter().any(|p| p.attr == attr) {
            return;
        }
        // The desktop ids the package ships (uninstall mapping + resolve
        // matching), recovered from the live package hits by attr. The
        // attr itself is a valid match key (some apps' id == attr).
        let hit = self.pkg_hits.iter().position(|p| p.attr == attr);
        let mut desktop_ids = hit
            .map(|h| self.pkg_hits[h].icons.clone())
            .unwrap_or_default();
        desktop_ids.push(attr.to_owned());
        // Reserve an icon slot in the pending-install texture block. If
        // every slot is taken, fall back to a plain install (the app still
        // lands, just without a placeheld tile).
        let Some(icon_slot) = (0..nix::PENDING_INSTALL_CAP)
            .find(|s| !self.pending_installs.iter().any(|p| &p.icon_slot == s))
        else {
            info!("pending-install slots full; installing {attr} without a tile");
            self.start_managed_install(attr, desktop_ids);
            return;
        };
        // The grid id the tile should sit before: the first item at or
        // past the drop slot (the make-room gap) on its display page.
        // A page-tail / ghost-page drop → append (no anchor). A dock or box
        // drop rides that surface instead, so it takes no grid anchor.
        let anchor = (dock_slot.is_none() && box_dest.is_none())
            .then(|| {
                self.reorder_slot.and_then(|slot| {
                    let cap = self.apps_cap.max(1);
                    let dp = slot / cap;
                    self.apps_slots
                        .iter()
                        .enumerate()
                        .filter(|&(_, &s)| s >= slot && s / cap == dp)
                        .min_by_key(|&(_, &s)| s)
                        .and_then(|(j, _)| self.search.visible[content::SECTION_APPS].get(j))
                        .and_then(|&e| self.entries.get(e))
                        .map(|e| e.id.clone())
                })
            })
            .flatten();
        // An anchorless grid drop still names its PAGE (see grid_page on
        // the struct): the storage page under the drop slot, or a fresh
        // page for the ghost-page gesture.
        let grid_page = (dock_slot.is_none() && box_dest.is_none() && anchor.is_none())
            .then(|| {
                self.reorder_slot.map(|slot| {
                    let dp = slot / self.apps_cap.max(1);
                    self.apps_page_map
                        .get(dp)
                        .copied()
                        .unwrap_or_else(|| self.order.pages().len())
                })
            })
            .flatten();
        // The package's own icon, recovered from the hits by attr; no
        // rasterized hit icon falls back to the generic package tile.
        let (icon_pixels, placeholder) = match hit {
            Some(h) => (
                self.pkg_hit_icons.get(h).cloned().unwrap_or_default(),
                self.pkg_hit_placeholders.get(h).copied().unwrap_or(true),
            ),
            None => (Vec::new(), true),
        };
        if !icon_pixels.is_empty() {
            let layer = self.pkg_layer_base + nix::RANK_HITS_MAX as u32 + icon_slot as u32;
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.update_icon_layer(layer, &icon_pixels);
            }
        }
        info!(
            "installing {attr} (slot {icon_slot}, anchor {anchor:?}, page {grid_page:?}, dock {dock_slot:?})"
        );
        self.pending_installs.push(PendingInstall {
            attr: attr.to_owned(),
            name,
            version,
            desktop_ids: desktop_ids.clone(),
            anchor,
            grid_page,
            dock_slot,
            box_dest,
            icon_slot,
            icon_pixels,
            placeholder,
            failed: false,
            started: Instant::now(),
            completed_at: None,
            rescan_fired: false,
        });
        // A dock drop was already pinned at its slot by the caller (via
        // `pin_dropped_on_dock`), so the tile shows there immediately; the
        // stored `dock_slot` lets the resolve re-pin the finished app.
        // Stage in memory only — packages.nix is NOT written until Done ok=true.
        self.managed.stage(attr, desktop_ids);
        self.recompute_removable();
        self.busy_ids.insert(attr.to_owned());
        self.nix.request(nix::Request::Install {
            id: attr.to_owned(),
            attr: attr.to_owned(),
        });
        self.save_install_state();
        self.refilter();
    }

    /// Drop a pending-install tile (dismissed by the user, or resolved),
    /// freeing its icon slot and clearing any lingering busy mark.
    pub(crate) fn remove_pending(&mut self, attr: &str) {
        // A box-destined tile placeholds its slot in the box under `attr`
        // until it resolves. Dismissing a failed one must also pull that
        // member now — otherwise the box keeps a phantom slot (and a blank
        // mini) until the next rescan's prune happens to clear it.
        let boxed = self
            .pending_installs
            .iter()
            .find(|p| p.attr == attr)
            .and_then(|p| p.box_dest.clone());
        self.pending_installs.retain(|p| p.attr != attr);
        self.busy_ids.remove(attr);
        if boxed.is_some() && self.groups.remove_member(attr) {
            // The box may have just dissolved (dropped below two members):
            // forget its remembered grid slot so nothing dead lingers.
            let live: std::collections::HashSet<String> = self
                .groups
                .groups()
                .iter()
                .map(|g| format!("group:{}", g.id))
                .collect();
            self.order.forget_dead_boxes(&live);
        }
        self.save_install_state();
        self.refilter();
    }

    /// Append a transient Apps-grid cell for each pending install and slot
    /// it into `cells` just before its anchor id (end if the anchor is
    /// gone), keeping `slots` parallel: the tile takes the anchor's display
    /// slot and the rest of that page shifts one right (a full page's last
    /// item may transiently spill onto the next page start; the resolve
    /// refilter settles it). Mirrors [`Self::push_transient_pkg`]: the
    /// entry id is the attr, so the busy / failed / installing flags and
    /// retry-on-click all key off it.
    pub(crate) fn insert_pending_cells(&mut self, cells: &mut Vec<usize>, slots: &mut Vec<usize>) {
        for i in 0..self.pending_installs.len() {
            let p = &self.pending_installs[i];
            let (attr, name, version, anchor, slot, has_icon, ph) = (
                p.attr.clone(),
                p.name.clone(),
                p.version.clone(),
                p.anchor.clone(),
                p.icon_slot,
                !p.icon_pixels.is_empty(),
                p.placeholder,
            );
            let (layer, placeholder) = if has_icon {
                (
                    self.pkg_layer_base + nix::RANK_HITS_MAX as u32 + slot as u32,
                    ph,
                )
            } else {
                self.asset("asset-pkg").unwrap_or((0, true))
            };
            let entry = AppEntry {
                id: attr,
                name,
                description: Some(version),
                exec: String::new(),
                icon: None,
                startup_wm_class: None,
                needs_terminal: false,
                path: None,
            };
            let idx = self.push_transient(entry, apps::EntryKind::Package, placeholder, layer);
            // A dock- or box-destined tile renders on that surface (its dock
            // pin, or as a box member), not in the grid — the transient entry
            // still exists so it can be drawn, but it claims no grid cell.
            if self.pending_installs[i].dock_slot.is_some()
                || self.pending_installs[i].box_dest.is_some()
            {
                continue;
            }
            let cap = self.apps_cap.max(1);
            match anchor
                .as_ref()
                .and_then(|a| cells.iter().position(|&e| self.entries[e].id == *a))
            {
                Some(at) => {
                    let s = slots.get(at).copied().unwrap_or(at);
                    let page = s / cap;
                    cells.insert(at, idx);
                    slots.insert(at, s);
                    // Shift the rest of the anchor's page one right
                    // (within-page slots are dense, so stop at the first
                    // slot on another page).
                    for sk in slots[at + 1..].iter_mut() {
                        if *sk / cap != page {
                            break;
                        }
                        *sk += 1;
                    }
                }
                None => {
                    // An anchorless grid drop still targets its page
                    // (grid_page): the tile sits at that display page's
                    // end. Pushing it to the grid's last cell rendered it
                    // on the LAST page — the visible half of the page-2
                    // teleport (the resolve's landing was the other).
                    // A grid_page not in the map is the ghost page: one
                    // past the last display page, same formula.
                    let dp = self.pending_installs[i].grid_page.map(|sp| {
                        self.apps_page_map
                            .iter()
                            .position(|&p| p == sp)
                            .unwrap_or(self.apps_page_map.len())
                    });
                    match dp {
                        Some(dp) => {
                            let at = slots.partition_point(|&s| s / cap <= dp);
                            let s = if at > 0 && slots[at - 1] / cap == dp {
                                slots[at - 1] + 1
                            } else {
                                dp * cap
                            };
                            cells.insert(at, idx);
                            slots.insert(at, s);
                        }
                        None => {
                            slots.push(slots.last().map_or(0, |s| s + 1));
                            cells.push(idx);
                        }
                    }
                }
            }
        }
    }

    /// After a profile rescan, retire any pending install whose app has
    /// now appeared: land the real app in the tile's grid slot (before
    /// its captured anchor), keep it in the box rather than the dock, and
    /// bounce it in. Failed tiles are left for the user to retry/dismiss.
    ///
    /// Matching an install to its app is deliberately layered, because a
    /// package's attr often differs from the desktop id it ships and the
    /// index may carry no desktop hints at all (`chromium` installs
    /// `chromium-browser`): (1) an exact desktop-id hit, then (2) an app
    /// that *newly appeared* since the last scan whose name relates to
    /// the attr, then (3) the lone new app when a single install is
    /// outstanding.
    pub(crate) fn resolve_pending_installs(&mut self) {
        // Real `.desktop` apps only — the synthetic CLI tiles are excluded
        // so a pending install can't latch onto the placeholder whose id
        // equals the attr (a `chromium` tile shadowing the real
        // `chromium-browser`). CLI-only tools resolve to their tile via the
        // dedicated fallback below.
        let current: HashSet<String> = self
            .entries
            .iter()
            .zip(&self.kinds)
            .take(self.base_len)
            .filter(|(e, k)| **k == apps::EntryKind::App && !self.cli_ids.contains(&e.id))
            .map(|(e, _)| e.id.clone())
            .collect();
        let newly: Vec<String> = current.difference(&self.known_app_ids).cloned().collect();

        // Only resolve installs that have actually completed: a package
        // still building (busy) hasn't got its real `.desktop` yet, so it
        // must wait for its own rescan rather than latch onto whatever the
        // scan currently holds (another package's app, or a phantom).
        let pending: Vec<(String, Vec<String>, Option<String>)> = self
            .pending_installs
            .iter()
            .filter(|p| !p.failed && !self.busy_ids.contains(&p.attr))
            // Hold a just-finished tile until its ring fill and shine sweep
            // have both played, so the completion flourish always finishes
            // before the swap-in.
            .filter(|p| p.completed_at.is_none_or(|c| c.elapsed() >= INSTALL_HOLD))
            .map(|p| (p.attr.clone(), p.desktop_ids.clone(), p.anchor.clone()))
            .collect();
        // (attr, app_id, anchor, gui) — gui is false when the package
        // resolved to its own CLI tile (a genuine command-line tool), true
        // when it resolved to a scanned GUI `.desktop`.
        let mut resolved: Vec<(String, String, Option<String>, bool)> = Vec::new();
        let mut claimed: HashSet<String> = HashSet::new();
        for (attr, desktop_ids, anchor) in &pending {
            let hit = desktop_ids
                .iter()
                .find(|d| current.contains(d.as_str()) && !claimed.contains(d.as_str()))
                .cloned()
                .or_else(|| {
                    // A freshly-appeared app whose name relates to the attr.
                    let key = normalize_id(attr);
                    newly
                        .iter()
                        .find(|id| {
                            !claimed.contains(id.as_str()) && {
                                let n = normalize_id(id);
                                n.contains(&key) || key.contains(&n)
                            }
                        })
                        .cloned()
                });
            if let Some(app_id) = hit {
                claimed.insert(app_id.clone());
                resolved.push((attr.clone(), app_id, anchor.clone(), true));
            }
        }
        // Last resort: a single outstanding install ↔ a single new app
        // (covers reverse-DNS ids that share no substring with the attr).
        let still: Vec<&(String, Vec<String>, Option<String>)> = pending
            .iter()
            .filter(|(a, _, _)| !resolved.iter().any(|(ra, _, _, _)| ra == a))
            .collect();
        let leftover: Vec<&String> = newly.iter().filter(|id| !claimed.contains(*id)).collect();
        if let ([(attr, _, anchor)], [app_id]) = (still.as_slice(), leftover.as_slice()) {
            claimed.insert((*app_id).clone());
            resolved.push((attr.clone(), (*app_id).clone(), anchor.clone(), true));
        }
        // CLI-only fallback: a pending install that matched no scanned GUI
        // app but has a synthetic CLI tile (id == attr) is a command-line
        // tool — resolve it to that tile so it lands and stops "installing".
        for (attr, _, anchor) in &pending {
            if resolved.iter().any(|(ra, _, _, _)| ra == attr) {
                continue;
            }
            if self.cli_ids.contains(attr) {
                resolved.push((attr.clone(), attr.clone(), anchor.clone(), false));
            }
        }

        let mut linked_real = false;
        let mut changed = false;
        let any_resolved = !resolved.is_empty();
        for (attr, app_id, anchor, gui) in resolved {
            // Where the tile was dropped decides where the app lands: a dock
            // drop re-pins the finished app at the same slot; a box drop
            // swaps the placeholder member for the real app in place; a grid
            // drop keeps it in the grid at its anchor and never pins — so it
            // stays exactly where it was dropped instead of teleporting.
            let (dock_slot, box_dest, grid_page) = self
                .pending_installs
                .iter()
                .find(|p| p.attr == attr)
                .map(|p| (p.dock_slot, p.box_dest.clone(), p.grid_page))
                .unwrap_or((None, None, None));
            if let Some(anchor) = anchor {
                self.order.insert_before(&app_id, &anchor);
            } else if let Some(page) = grid_page {
                // Page-tail / ghost-page drop: land at the end of the page
                // the user dropped on — the order's default (last page)
                // teleported end-of-page-1 drops onto page 2.
                self.order.move_to_page_end(&app_id, page);
            }
            // Record the real app id and whether it is a GUI app, so a
            // wrapped package (chromium → chromium-browser) never regrows a
            // phantom terminal tile and uninstall maps back correctly.
            self.managed
                .note_installed(&attr, std::slice::from_ref(&app_id), gui);
            changed = true;
            linked_real |= gui;
            // A plain grid install (not dropped on the dock or into a box).
            let grid_dest = dock_slot.is_none() && box_dest.is_none();
            if let Some(slot) = dock_slot {
                self.pins.unpin(&attr);
                self.pins.pin_at(&app_id, slot);
            }
            if let Some(box_id) = box_dest {
                if let Some(g) = self.groups.index_by_id(&box_id) {
                    self.groups.replace_member(g, &attr, &app_id);
                }
            }
            // A grid- or dock-destined install that finished while the launcher
            // was hidden: the whole ring-fill + shine flourish just played on a
            // hidden surface, so pop the dock up and replay a one-shot shine on
            // the app. A grid drop temp-pins the app onto the dock until it's
            // opened and closed (see `reconcile_install_notify`); a dock drop's
            // app is already pinned at its slot, so the notify only drives the
            // shine and self-cleans. (A box drop stays quiet — its app lives
            // inside the box, not on the dock.)
            if (grid_dest || dock_slot.is_some()) && self.ui.target() != Target::Open {
                self.install_notify.push(InstallNotify {
                    id: app_id.clone(),
                    shine_at: std::time::Instant::now(),
                    seen_running: false,
                });
                if self.ui.apply(Command::Show) {
                    // Popped up from hidden just for the shine: auto-hide in 3s.
                    self.arm_notify_dock_hide();
                }
            } else {
                self.just_installed = Some(app_id.clone()); // bounce it in place
            }
            info!(
                "pending install {attr} resolved as app {app_id} (gui={gui}, dock {dock_slot:?})"
            );
            self.pending_installs.retain(|p| p.attr != attr);
            self.busy_ids.remove(&attr);
        }

        // Also resolve managed installs that had no grid tile (dock-drop
        // path and startup recovery).  Pin any newly-appeared app whose
        // desktop ids or name matches the attr, else its CLI tile.
        let mut resolved_managed: Vec<String> = Vec::new();
        for attr in self.managed_install_attrs.clone() {
            if self.busy_ids.contains(&attr) {
                continue; // still installing — wait for its own rescan
            }
            let desktop_ids = self.managed.desktop_ids_for(&attr);
            let hit = desktop_ids
                .iter()
                .find(|d| current.contains(d.as_str()) && !claimed.contains(d.as_str()))
                .cloned()
                .or_else(|| {
                    let key = normalize_id(&attr);
                    newly
                        .iter()
                        .find(|id| {
                            !claimed.contains(id.as_str()) && {
                                let n = normalize_id(id);
                                n.contains(&key) || key.contains(&n)
                            }
                        })
                        .cloned()
                });
            let (app_id, gui) = match hit {
                Some(app_id) => (app_id, true),
                None if self.cli_ids.contains(&attr) => (attr.clone(), false),
                None => continue,
            };
            claimed.insert(app_id.clone());
            self.managed
                .note_installed(&attr, std::slice::from_ref(&app_id), gui);
            changed = true;
            linked_real |= gui;
            let slot = self.pins.pins().len();
            self.pins.pin_at(&app_id, slot);
            self.just_installed = Some(app_id.clone());
            self.busy_ids.remove(&attr);
            info!("managed install {attr} resolved as app {app_id} (gui={gui})");
            resolved_managed.push(attr.clone());
        }
        self.managed_install_attrs
            .retain(|a| !resolved_managed.contains(a));
        if any_resolved || !resolved_managed.is_empty() {
            self.save_install_state();
        }

        // Reconcile managed apps whose GUI-ness was never learned (adopted
        // from the package list on startup, or a migrated legacy entry):
        // link each to its real scanned app — matched as the attr,
        // optionally with a suffix, so a wrapped package (chromium →
        // chromium-browser) is recognized — so its phantom terminal tile
        // clears and uninstall can map back to it. A CLI-only tool finds no
        // such app and keeps its tile. These aren't fresh installs, so they
        // are never pinned; ones handled by the pending / dock-drop flows
        // above are skipped.
        for attr in self.managed.gui_unknown_attrs() {
            if self.busy_ids.contains(&attr) || self.pending_installs.iter().any(|p| p.attr == attr)
            {
                continue;
            }
            let key = normalize_id(&attr);
            let hit = current
                .iter()
                .find(|id| {
                    !claimed.contains(id.as_str()) && {
                        let n = normalize_id(id);
                        n == key || n.starts_with(&key)
                    }
                })
                .cloned();
            match hit {
                Some(app_id) => {
                    claimed.insert(app_id.clone());
                    self.managed
                        .note_installed(&attr, std::slice::from_ref(&app_id), true);
                    changed = true;
                    linked_real = true;
                }
                None if self.cli_ids.contains(&attr) => {
                    self.managed.note_installed(&attr, &[], false);
                    changed = true;
                }
                None => {} // its app may simply not be scanned yet — retry next scan
            }
        }

        // note_installed persisted the newly-learned app ids / gui flags;
        // just refresh the removable set to match.
        if changed {
            self.recompute_removable();
        }
        // A GUI app resolved: a stale phantom CLI tile for it may still be in
        // this scan's entries — rescan to drop it now that its gui flag is set.
        if linked_real {
            self.indexer.request_rescan();
        }

        // F13 drift sweep, once per daemon lifetime on the first (non-empty)
        // scan: a confirmed GUI package none of whose apps are live means the
        // profile drifted under a truthful status — a boot into an older
        // generation, or state left by a crash. One forced apply
        // rematerializes the whole list (which is why a single nudge covers
        // every drifted attr). Skipped when a startup path already armed an
        // apply — that run reconciles everything on its own.
        if !self.reconciled && !current.is_empty() {
            self.reconciled = true;
            let drifted: Vec<String> = self
                .managed
                .confirmed_gui_attrs()
                .into_iter()
                .filter(|(attr, ids)| {
                    !self.busy_ids.contains(attr)
                        && !self.pending_installs.iter().any(|p| &p.attr == attr)
                        && !self.uninstalling.values().any(|a| a == attr)
                        && !ids.iter().any(|d| current.contains(d))
                })
                .map(|(attr, _)| attr)
                .collect();
            if !drifted.is_empty() {
                warn!(
                    "reconcile: installed packages with no live app: {}; forcing one apply",
                    drifted.join(", ")
                );
                self.nix
                    .request(nix::Request::EnsureApplied { force: true });
            }
        }

        self.known_app_ids = current;
    }

    /// Re-upload every pending install's icon into its reserved texture
    /// slot after `set_icons` rebuilt the array (which wiped the tail).
    pub(crate) fn reupload_pending_icons(&mut self) {
        let base = self.pkg_layer_base + nix::RANK_HITS_MAX as u32;
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for p in self.pending_installs.iter() {
            if !p.icon_pixels.is_empty() {
                renderer.update_icon_layer(base + p.icon_slot as u32, &p.icon_pixels);
            }
        }
    }

    /// Persist the in-flight install state (pending tiles + tile-less
    /// managed installs) so a daemon restart can restore and re-arm it.
    /// Call after every material change: start, retry, fail, dismiss,
    /// resolve. With nothing in flight the file is removed. Icon sidecars
    /// are written once per tile and stale ones swept out.
    pub(crate) fn save_install_state(&self) {
        let path = pending_state_path();
        let state = SavedInstallState {
            tiles: self
                .pending_installs
                .iter()
                .map(|p| SavedPending {
                    attr: p.attr.clone(),
                    name: p.name.clone(),
                    version: p.version.clone(),
                    desktop_ids: p.desktop_ids.clone(),
                    anchor: p.anchor.clone(),
                    grid_page: p.grid_page,
                    dock_slot: p.dock_slot,
                    box_dest: p.box_dest.clone(),
                    started_epoch: epoch_now() - p.started.elapsed().as_secs_f64(),
                    failed: p.failed,
                    placeholder: p.placeholder,
                })
                .collect(),
            managed: self.managed_install_attrs.clone(),
        };
        if state.tiles.is_empty() && state.managed.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            crate::persist::write_json("pending", &path, &state);
        }
        for p in &self.pending_installs {
            let icon = pending_icon_path(&p.attr);
            if !p.icon_pixels.is_empty() && !icon.exists() {
                crate::persist::write_bytes("pending", &icon, &p.icon_pixels);
            }
        }
        // Sweep icon sidecars whose tile is gone (resolved / dismissed).
        if let Some(dir) = path.parent() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let Some(attr) = name
                        .to_str()
                        .and_then(|n| n.strip_prefix("pending-icon-"))
                        .and_then(|n| n.strip_suffix(".rgba"))
                    else {
                        continue;
                    };
                    if !self.pending_installs.iter().any(|p| p.attr == attr) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    /// Restore installs that were mid-flight when the daemon last stopped:
    /// recreate each pending tile at its saved placement, re-stage it in the
    /// managed cache (unconfirmed — it never reaches `managed.json` before
    /// its rebuild lands), and re-arm the install through the normal
    /// mutation path, whose status-file rules make the re-arm exact: a
    /// rebuild that finished while we were dead fast-completes, a
    /// still-running one is joined, a failed one retries once. A tile saved
    /// as failed is restored failed (click retries), not re-armed.
    ///
    /// Returns whether any install was re-armed — the caller then skips the
    /// startup reconcile, since the re-armed apply proves the whole list.
    pub(crate) fn restore_install_state(&mut self) -> bool {
        let Some(state) = crate::persist::read_json::<SavedInstallState>(&pending_state_path())
        else {
            return false;
        };
        let mut rearmed = false;
        for (slot, t) in state
            .tiles
            .into_iter()
            .take(nix::PENDING_INSTALL_CAP)
            .enumerate()
        {
            if self.pending_installs.iter().any(|p| p.attr == t.attr) {
                continue;
            }
            let icon_pixels = std::fs::read(pending_icon_path(&t.attr)).unwrap_or_default();
            // Resume the ring from where the attempt actually started
            // (clamped sane; a bogus epoch just restarts it).
            let elapsed = (epoch_now() - t.started_epoch).clamp(0.0, 86_400.0);
            let started = Instant::now()
                .checked_sub(Duration::from_secs_f64(elapsed))
                .unwrap_or_else(Instant::now);
            info!("restoring pending install {} (failed={})", t.attr, t.failed);
            self.managed.stage(&t.attr, t.desktop_ids.clone());
            if !t.failed {
                self.busy_ids.insert(t.attr.clone());
                self.nix.request(nix::Request::Install {
                    id: t.attr.clone(),
                    attr: t.attr.clone(),
                });
                rearmed = true;
            }
            self.pending_installs.push(PendingInstall {
                attr: t.attr,
                name: t.name,
                version: t.version,
                desktop_ids: t.desktop_ids,
                anchor: t.anchor,
                grid_page: t.grid_page,
                dock_slot: t.dock_slot,
                box_dest: t.box_dest,
                icon_slot: slot,
                placeholder: t.placeholder || icon_pixels.is_empty(),
                icon_pixels,
                failed: t.failed,
                started,
                completed_at: None,
                rescan_fired: false,
            });
        }
        for attr in state.managed {
            if self.busy_ids.contains(&attr) || self.managed_install_attrs.contains(&attr) {
                continue;
            }
            info!("restoring tile-less managed install {attr}");
            self.managed.stage(&attr, Vec::new());
            self.busy_ids.insert(attr.clone());
            self.managed_install_attrs.push(attr.clone());
            self.nix.request(nix::Request::Install {
                id: attr.clone(),
                attr,
            });
            rearmed = true;
        }
        rearmed
    }
}

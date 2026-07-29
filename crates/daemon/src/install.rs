//! The install flow: drag-to-install pending tiles, resolving finished
//! installs to their real apps, uninstall bookkeeping, the "try it"
//! launch, and the nix worker's event handling.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use tracing::{debug, error, info, warn};
use waverunner_core::index::AppEntry;

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
                        // behind the shared StandardOS banner naming the
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
                    // Profile changed: rescan so the grid gains the entry.
                    // Fresh variant clears stale icon-not-found cache entries
                    // so the newly installed app's icon loads.
                    self.indexer.request_rescan_fresh();
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
        let anchor = (dock_slot.is_none() && box_dest.is_none()).then(|| {
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
        }).flatten();
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
        info!("installing {attr} (slot {icon_slot}, anchor {anchor:?}, dock {dock_slot:?})");
        self.pending_installs.push(PendingInstall {
            attr: attr.to_owned(),
            name,
            version,
            desktop_ids: desktop_ids.clone(),
            anchor,
            dock_slot,
            box_dest,
            icon_slot,
            icon_pixels,
            placeholder,
            failed: false,
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
        self.refilter();
    }

    /// Drop a pending-install tile (dismissed by the user, or resolved),
    /// freeing its icon slot and clearing any lingering busy mark.
    pub(crate) fn remove_pending(&mut self, attr: &str) {
        self.pending_installs.retain(|p| p.attr != attr);
        self.busy_ids.remove(attr);
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
                    slots.push(slots.last().map_or(0, |s| s + 1));
                    cells.push(idx);
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
        for (attr, app_id, anchor, gui) in resolved {
            // Where the tile was dropped decides where the app lands: a dock
            // drop re-pins the finished app at the same slot; a box drop
            // swaps the placeholder member for the real app in place; a grid
            // drop keeps it in the grid at its anchor and never pins — so it
            // stays exactly where it was dropped instead of teleporting.
            let (dock_slot, box_dest) = self
                .pending_installs
                .iter()
                .find(|p| p.attr == attr)
                .map(|p| (p.dock_slot, p.box_dest.clone()))
                .unwrap_or((None, None));
            if let Some(anchor) = anchor {
                self.order.insert_before(&app_id, &anchor);
            }
            // Record the real app id and whether it is a GUI app, so a
            // wrapped package (chromium → chromium-browser) never regrows a
            // phantom terminal tile and uninstall maps back correctly.
            self.managed
                .note_installed(&attr, std::slice::from_ref(&app_id), gui);
            changed = true;
            linked_real |= gui;
            if let Some(slot) = dock_slot {
                self.pins.unpin(&attr);
                self.pins.pin_at(&app_id, slot);
            }
            if let Some(box_id) = box_dest {
                if let Some(g) = self.groups.index_by_id(&box_id) {
                    self.groups.replace_member(g, &attr, &app_id);
                }
            }
            self.just_installed = Some(app_id.clone());
            info!("pending install {attr} resolved as app {app_id} (gui={gui}, dock {dock_slot:?})");
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
            if self.busy_ids.contains(&attr)
                || self.pending_installs.iter().any(|p| p.attr == attr)
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
}

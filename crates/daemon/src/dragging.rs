//! Grid and dock drag & drop: the continuous make-room gap, edge paging
//! while a drag holds at a side, fold targets (app-onto-app boxes and dock
//! folders), and the drop resolution for every drag origin.

use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::{apps, content, groups, nix, pages};
use crate::{App, DragState, MAG_SLEEP_AFTER_DROP};

/// Minimum time between page turns while *holding a drag* at the grid /
/// box edge — slower than the scroll cooldown so held-edge paging steps
/// one page at a controllable pace instead of rapid-firing past them.
pub(crate) const DRAG_PAGE_COOLDOWN: Duration = Duration::from_millis(700);

/// Fraction of a pageable surface's width at each side that forms the
/// drag edge-paging band. One knob for the Apps grid and the open box.
pub(crate) const EDGE_PAGE_BAND: f32 = 0.14;

/// Which way an edge-held drag should page: -1 / +1 when `x` sits inside
/// the left / right [`EDGE_PAGE_BAND`] of the surface spanning
/// `rect_x..rect_x + rect_w`, else 0. The bands are forgiving: anything
/// at-or-past an edge counts (mid-drag overshoot still pages).
pub(crate) fn edge_page_dir(rect_x: f32, rect_w: f32, x: f32) -> i64 {
    let band = rect_w * EDGE_PAGE_BAND;
    if x < rect_x + band {
        -1
    } else if x > rect_x + rect_w - band {
        1
    } else {
        0
    }
}

/// Drive a hold-at-the-edge dwell clock (shared by the grid and box drag
/// paging): entering the band arms it, leaving disarms it, and while held
/// it fires — returns true — every [`DRAG_PAGE_COOLDOWN`]. Callers keep
/// frames coming while `dir != 0` (a stationary pointer emits no motion).
pub(crate) fn edge_page_due(timer: &mut Option<Instant>, dir: i64) -> bool {
    if dir == 0 {
        *timer = None;
        return false;
    }
    match timer {
        Some(t) if t.elapsed() >= DRAG_PAGE_COOLDOWN => {
            *timer = Some(Instant::now());
            true
        }
        Some(_) => false,
        None => {
            *timer = Some(Instant::now());
            false
        }
    }
}

/// Horizontal band within a cell (fraction of its width) where a drop onto
/// an app makes/joins a box. The centre ~40% folds; the outer edges (the
/// seam between two icons) reorder instead.
pub(crate) const FOLD_BAND: std::ops::Range<f32> = 0.30..0.70;

impl App {
    /// Which dock slot a drag should insert before, given the pointer
    /// position.  Returns `None` when the pointer is outside the dock band.
    pub(crate) fn drag_dock_insert(
        &self,
        layout: &content::Layout,
        pos: (f32, f32),
    ) -> Option<usize> {
        let (x, y) = pos;
        let slots = &layout.dock_slots;
        let dock_top = layout.card_top;
        // An empty dock has no slots to measure against, but the band is
        // still there — fall back to its height so a drop anywhere in it
        // pins at the front (slot 0). Without this the user can never
        // re-pin an app once the dock has been cleared.
        let dock_bottom = match slots.first() {
            Some(s) => s.y + s.h,
            None => dock_top + self.config.window.input_bar_height as f32,
        };
        if y < dock_top || y > dock_bottom {
            return None;
        }
        let insert = slots
            .iter()
            .position(|s| x < s.x + s.w / 2.0)
            .unwrap_or(slots.len());
        Some(insert)
    }

    /// The dock slot whose icon the pointer is centered over — a fold
    /// target (drop an app on it to join/create a box). `None` between
    /// icons or off the dock.
    pub(crate) fn dock_fold_target(
        &self,
        layout: &content::Layout,
        pos: (f32, f32),
    ) -> Option<usize> {
        let (x, y) = pos;
        if y < layout.card_top || y > layout.dock_hit_bottom {
            return None;
        }
        layout.dock_slots.iter().position(|s| {
            let fx = (x - s.x) / s.w;
            (0.25..0.75).contains(&fx)
        })
    }

    /// Drop an app centered on a dock icon: add it to a folder there, or
    /// create a new folder (pinned in the target app's slot) from the two
    /// apps. Returns whether it folded.
    pub(crate) fn handle_dock_fold(
        &mut self,
        dragged_idx: usize,
        dragged_id: &str,
        pos: (f32, f32),
    ) -> bool {
        let layout = self.current_layout();
        let Some(slot) = self.dock_fold_target(&layout, pos) else {
            return false;
        };
        let Some(&target_idx) = self.dock_order.get(slot) else {
            return false;
        };
        if target_idx == dragged_idx {
            return false; // dropped on itself
        }
        let target_id = self.entries[target_idx].id.clone();
        // Dropped onto the Recycle Bin: uninstall the app (→ back to Install)
        // instead of folding it into a box. A non-removable app is a no-op and
        // simply snaps back.
        if groups::is_trash(&target_id) {
            self.uninstall_app(dragged_id);
            return true;
        }
        match self.kinds.get(target_idx) {
            // Join the folder.
            Some(apps::EntryKind::Group) => {
                let Some(g) = target_id
                    .strip_prefix("group:")
                    .and_then(|gid| self.groups.index_by_id(gid))
                else {
                    return false;
                };
                self.groups.add(g, dragged_id);
                self.pins.unpin(dragged_id); // if it rode the dock
                self.refilter();
                true
            }
            // Create a folder of [target, dragged], pinned where target sat.
            Some(apps::EntryKind::App) => {
                let box_pin = format!("group:{}", self.groups.create(&target_id, dragged_id));
                let at = self
                    .pins
                    .pins()
                    .iter()
                    .position(|p| p == &target_id)
                    .unwrap_or(self.pins.pins().len());
                self.pins.pin_at(&box_pin, at);
                self.pins.unpin(&target_id);
                self.pins.unpin(dragged_id);
                self.refilter();
                true
            }
            _ => false,
        }
    }

    /// Uninstall the app `id` if it's removable — a waverunner-managed package
    /// app (a declarative `nixos-rebuild`) or an installed catalog webapp.
    /// Returns whether an uninstall was actually started. Shared by the
    /// Install-section drop and the Recycle Bin drop.
    ///
    /// A managed app defers dropping its cache entry / dock pin until the
    /// rebuild's `Done` (a failed rebuild re-adds the package-list line, so
    /// tearing down now would diverge the two); its cell shows busy meanwhile.
    /// A webapp is instant. A non-removable (base/system) app is a no-op.
    pub(crate) fn uninstall_app(&mut self, id: &str) -> bool {
        if self.busy_ids.contains(id) {
            return false;
        }
        if self.removable_ids.contains(id) {
            if let Some(attr) = self.managed.attr_for_app(id) {
                info!("uninstalling {id} (attr {attr})");
                self.kill_app_windows(id);
                self.busy_ids.insert(id.to_owned());
                self.uninstalling.insert(id.to_owned(), attr.clone());
                self.nix.request(nix::Request::Remove {
                    id: id.to_owned(),
                    attr,
                });
                return true;
            }
        }
        if let Some(slug) = self.installed_webapp_slug(id) {
            self.uninstall_webapp(&slug);
            return true;
        }
        false
    }

    /// Whether `pos` is over the Recycle Bin's dock tile (its centre — the
    /// same fold-target test app drops use).
    pub(crate) fn dropped_on_trash(&self, layout: &content::Layout, pos: (f32, f32)) -> bool {
        self.dock_fold_target(layout, pos)
            .and_then(|slot| self.dock_order.get(slot))
            .and_then(|&idx| self.entries.get(idx))
            .is_some_and(|e| groups::is_trash(&e.id))
    }

    /// Send `path` (a Files-section entry's absolute-path id) to the
    /// FreeDesktop trash, then refilter so it drops out of the Files listing
    /// and, if the trash view is open, appears in the bin.
    pub(crate) fn trash_file(&mut self, path: &str) {
        match crate::trash::Trash::home().trash(std::path::Path::new(path)) {
            Ok(item) => info!("trashed {path} → Trash/files/{}", item.name),
            Err(e) => warn!("failed to trash {path}: {e}"),
        }
        self.refilter();
    }

    /// Finish a drag. Apps pin at the dock slot they were dropped on
    /// (dock-origin drags dropped elsewhere unpin), a profile app
    /// dropped on the Install section (or the Recycle Bin) uninstalls, and
    /// a package dropped on the Apps section (or the dock) installs.
    /// `released` is false when the drag ended by the pointer leaving the
    /// surface — that path never installs or uninstalls anything.
    pub(crate) fn drop_drag(&mut self, drag: DragState, insert: Option<usize>, released: bool) {
        let Some(entry) = self.entries.get(drag.entry_idx) else {
            return; // entries were replaced mid-drag
        };
        let id = entry.id.clone();
        // A package's label / version, needed if this drop starts a
        // drag-to-install grid tile (the entry vanishes on the next
        // refilter, so snapshot them now).
        let pkg_name = entry.name.clone();
        let pkg_version = entry.description.clone().unwrap_or_default();
        let kind = self.kinds.get(drag.entry_idx).copied();
        // Whatever this drop rearranges, the dragged icon itself must
        // land in its chosen cell, not glide there from its origin: the
        // next refilter places it at rest (self-clears after use).
        self.just_dropped = Some(id.clone());
        let layout = self.current_layout();
        let section = if released {
            content::section_at(&layout, drag.pos)
        } else {
            None
        };
        // Visual snapshot (absolute/display positions) of everything
        // that might rearrange: any unfinished make-room glide then
        // completes smoothly across the drop instead of snapping.
        let dock_vis: Vec<(usize, f32)> = self
            .dock_order
            .iter()
            .enumerate()
            .take(layout.dock_slots.len())
            .filter(|(_, &e)| e != drag.entry_idx)
            .map(|(k, &e)| {
                let slot = &layout.dock_slots[k];
                let shift = self.dock_slide.get(k).copied().unwrap_or(0.0);
                (e, slot.x + slot.w / 2.0 + shift * slot.w)
            })
            .collect();
        debug!(
            "drop: id={id} kind={kind:?} from_dock={} insert={insert:?} section={section:?}",
            drag.from_dock
        );
        match kind {
            Some(apps::EntryKind::Package) => {
                // An existing pending tile being re-dragged: only a
                // *failed* one can be dismissed by dragging it clear of
                // the grid (a still-installing tile snaps back — the
                // install can't be cancelled). Never on the pointer-left
                // path (`released` false), which must not touch it.
                let outside = released && self.outside_card(&layout, drag.pos);
                if let Some(failed) = self
                    .pending_installs
                    .iter()
                    .find(|p| p.attr == id)
                    .map(|p| p.failed)
                {
                    // A re-dragged tile is dismissed by dropping it clear
                    // of the box (only a failed one — a running install
                    // can't be cancelled).
                    if failed && outside {
                        self.remove_pending(&id);
                    }
                } else if outside && !self.busy_ids.contains(&id) {
                    // Dropped clear of the box: run it ephemerally without
                    // installing (terminal-vs-GUI decided after the build).
                    // Checked before the install cases because `section_at`
                    // is Y-only — a margin drop at grid altitude still
                    // reports SECTION_APPS and would otherwise install.
                    self.start_launch(&id);
                } else if let Some(target) = (section == Some(content::SECTION_APPS))
                    .then(|| self.grid_fold_at(drag.pos, drag.entry_idx))
                    .flatten()
                {
                    // Dropped on an app/box centre: install into a box.
                    self.start_pending_install_boxed(&id, pkg_name, pkg_version, target);
                } else if section == Some(content::SECTION_APPS) {
                    // Fresh package dropped into the grid: place it as a
                    // tile at the drop slot and install it in place.
                    self.start_pending_install(&id, pkg_name, pkg_version, None, None);
                } else if let Some(slot) = insert {
                    if !self.busy_ids.contains(&id) {
                        // Dropped on the dock band: pin the tile at that slot
                        // and install in place; the finished app re-pins at
                        // the same slot (see `resolve_pending_installs`).
                        let pinned = self.pin_dropped_on_dock(&id, slot, None, false);
                        self.start_pending_install(&id, pkg_name, pkg_version, Some(pinned), None);
                    }
                }
            }
            Some(apps::EntryKind::App)
                if section == Some(content::SECTION_INSTALL)
                    && self.removable_ids.contains(&id)
                    && !self.busy_ids.contains(&id) =>
            {
                // Uninstall — gated to managed apps, so this only runs for
                // something the launcher installed (non-removable apps fall
                // through and snap back). See `uninstall_app`.
                self.uninstall_app(&id);
            }
            // A catalog webapp: dropped clear of the card runs it ephemerally
            // ("try it", window shows in the dock unpinned); dropped on the
            // grid installs it (moves out of the Install section onto the
            // grid). It is never on the grid, so it never reorders.
            Some(apps::EntryKind::App) if self.catalog_webapp_slug(&id).is_some() => {
                if released && self.outside_card(&layout, drag.pos) {
                    self.launch_webapp(drag.entry_idx);
                } else if let Some(target) = (section == Some(content::SECTION_APPS))
                    .then(|| self.grid_fold_at(drag.pos, drag.entry_idx))
                    .flatten()
                {
                    // Dropped on an app/box centre: install into a box.
                    if let Some(slug) = self.catalog_webapp_slug(&id) {
                        self.install_webapp_boxed(&slug, target, drag.entry_idx);
                    }
                } else if section == Some(content::SECTION_APPS) {
                    if let Some(slug) = self.catalog_webapp_slug(&id) {
                        self.install_webapp(&slug, drag.entry_idx);
                    }
                } else if let Some(slot) = insert {
                    // Dropped on the dock band: install and pin at that slot.
                    if let Some(slug) = self.catalog_webapp_slug(&id) {
                        self.install_webapp_on_dock(&slug, slot);
                    }
                }
                // Dropped back in the Install section / elsewhere: snap back.
            }
            // An installed webapp dragged into the Install section uninstalls
            // (returns to the catalog; the launcher file stays on disk).
            Some(apps::EntryKind::App)
                if section == Some(content::SECTION_INSTALL)
                    && self.installed_webapp_slug(&id).is_some() =>
            {
                self.uninstall_app(&id);
            }
            // A file or directory (from the Files section) dropped onto the
            // Recycle Bin: send it to the FreeDesktop trash. Its entry id is
            // the absolute path (see `push_transient_file`). It vanishes from
            // the Files listing; an open trash view picks it up on refilter.
            Some(apps::EntryKind::File) if released && self.dropped_on_trash(&layout, drag.pos) => {
                self.trash_file(&id);
            }
            _ => {
                // Grid gestures: an app dropped on another app creates a
                // box, on a box joins it, in a gap reorders (boxes too).
                // A dock app dragged into the grid unpins and lands there
                // the same way. Dropping out of the grid (and off the
                // dock) is a no-op — the icon snaps back to its place.
                let boxed = released
                    && insert.is_none()
                    && matches!(
                        kind,
                        Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
                    )
                    && self.grid_resting()
                    && self.handle_grid_drop(drag.entry_idx, &id, drag.pos);
                // Dropped centered on a dock icon: join/create a folder on
                // the dock (an app onto a folder joins; onto an app boxes).
                let dock_folded = !boxed
                    && released
                    && self.grid_resting()
                    && kind == Some(apps::EntryKind::App)
                    && self.handle_dock_fold(drag.entry_idx, &id, drag.pos);
                if !boxed && !dock_folded {
                    // Dropped on the dock pins the item (app or box) there.
                    if let Some(slot) = insert {
                        self.pin_dropped_on_dock(&id, slot, Some(drag.entry_idx), drag.from_dock);
                    } else if drag.from_dock && released && kind == Some(apps::EntryKind::File) {
                        // A pinned path (dir/file from the Files section)
                        // has no grid home to land in: dropping it
                        // anywhere off the dock unpins it.
                        info!("unpinning path {id} (dropped off the dock)");
                        self.pins.unpin(&id);
                    }
                    // insert == None otherwise: dropped outside the dock
                    // band — no unpin, the icon just returns to the dock.
                }
            }
        }
        self.reorder_slot = None;
        self.grid_drag_page_at = None;
        self.recompute_dock_order();
        // Remap the dock glide onto the new arrangement *before*
        // anything draws: every icon keeps its exact current visual
        // position and eases to rest — nothing snaps at the drop, and
        // icons already at their seat (the common case) don't move at
        // all. (The grid gets the same continuity inside refilter.)
        let new_layout = self.current_layout();
        let n_dock = new_layout.dock_slots.len();
        self.dock_slide = vec![0.0; n_dock];
        for (k, &e) in self.dock_order.iter().take(n_dock).enumerate() {
            if let Some(&(_, cx)) = dock_vis.iter().find(|&&(ve, _)| ve == e) {
                let slot = &new_layout.dock_slots[k];
                self.dock_slide[k] = (cx - (slot.x + slot.w / 2.0)) / slot.w;
            }
        }
        // Magnification sleeps a full second so the icon simply *is*
        // placed before any wave returns.
        self.mag_sleep = Some(Instant::now() + MAG_SLEEP_AFTER_DROP);
        // A drag out of the Install section held the search query so the
        // grid could rest under it; now the drop has landed, clear the box
        // so the popup returns to a clean resting state.
        if self.install_drag_reset {
            self.install_drag_reset = false;
            self.search.query.clear();
            self.search.open = false;
        }
        self.refilter();
    }

    /// The Apps display cell under `pos` plus the within-cell
    /// fractions, from static slot geometry (display slots never move;
    /// only items glide between them — that's what keeps targeting
    /// stable while everything animates).
    pub(crate) fn apps_display_cell(
        &self,
        layout: &content::Layout,
        pos: (f32, f32),
    ) -> Option<(usize, f32, f32)> {
        let sec = &layout.sections[content::SECTION_APPS];
        if !sec.viewport.contains(pos) || sec.n_pages == 0 {
            return None;
        }
        let page_w = sec.viewport.w.max(1.0);
        // The page is the pager's TARGET (where a turn is headed), not the
        // eased scroll's image: mid-flip the eased position still reads as
        // the outgoing page, so a drop right after an edge-hold page turn
        // used to land back on the page the drag came from (SH). Columns
        // are viewport-relative for the same reason; at rest both formulas
        // agree exactly.
        let page = self.scroll.per[content::SECTION_APPS]
            .page(page_w)
            .min(sec.n_pages.saturating_sub(1));
        // Cell size must match what's on screen — the grid renders at
        // `icon_scale`, so map the pointer through the *scaled* cell, not the
        // raw constants (otherwise every row/column is mis-counted at any
        // non-default icon size, and the make-room reacts on the wrong cell).
        let scale = self.icon_scale();
        let fx = (pos.0 - sec.viewport.x) / (content::GRID_CELL_W * scale);
        let fy = (pos.1 - sec.viewport.y) / (content::GRID_CELL_H * scale);
        let col = (fx.floor() as usize).min(sec.cols.saturating_sub(1));
        let row = (fy.floor() as usize).min(sec.rows.saturating_sub(1));
        let d = page * sec.cols * sec.rows + row * sec.cols + col;
        Some((d, fx.fract(), fy.fract().clamp(0.0, 1.0)))
    }

    /// Track the drag's grid target in display space. The make-room gap
    /// follows the pointer continuously to the insertion seam nearest it
    /// (Launchpad-style — the grid parts as you move); hovering an app's
    /// centre instead rings it as a fold target and rests the grid so it
    /// holds still to fold onto. Returns the fold target, if any; the gap
    /// itself lives in `self.reorder_slot`.
    pub(crate) fn update_grid_target(
        &mut self,
        layout: &content::Layout,
    ) -> Option<(usize, usize)> {
        let Some(drag) = self.gesture.dragging.as_ref() else {
            self.reorder_slot = None;
            self.grid_drag_page_at = None;
            return None;
        };
        let kind = self.kinds.get(drag.entry_idx).copied();
        let visible = &self.search.visible[content::SECTION_APPS];
        let (len, pos) = (visible.len(), drag.pos);
        let orig = visible.iter().position(|&v| v == drag.entry_idx);
        // Ways to target the grid: a grid-origin app/box reordering
        // itself, a dock-origin app dragged in to unpin it, or an
        // Install-section item — a package, or a catalog webapp
        // (`EntryKind::App`, but living in Install not the grid) — dragged
        // up to be placed as it installs. The inserting cases own no cell,
        // so the gap is a brand-new slot (nothing to vacate) and may land
        // one past the end (append).
        let is_catalog_webapp = self
            .entries
            .get(drag.entry_idx)
            .is_some_and(|e| self.catalog_webapp_slug(&e.id).is_some());
        let inserting =
            drag.from_dock || kind == Some(apps::EntryKind::Package) || is_catalog_webapp;
        // The grid must be at rest to target a slot — true on an empty query,
        // or while dragging an install out of the Install section (the grid
        // is held resting for exactly this).
        let grid_drag = self.grid_resting()
            && if inserting {
                // A dock box dragged into the grid inserts too (it unpins
                // and lands there).
                matches!(
                    kind,
                    Some(apps::EntryKind::App)
                        | Some(apps::EntryKind::Package)
                        | Some(apps::EntryKind::Group)
                )
            } else {
                orig.is_some()
                    && len > 0
                    && matches!(
                        kind,
                        Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
                    )
            };
        if !grid_drag {
            self.reorder_slot = None;
            self.grid_drag_page_at = None;
            return None;
        }
        // Edge-paging: holding the drag in the grid's edge band turns
        // the page every DRAG_PAGE_COOLDOWN, carrying the icon across
        // pages (shared band + dwell with the open-box drag). The cycle
        // is: real pages, then the single ghost page, then back around
        // to the first page — one empty page in the loop, never two
        // (Max wants the cycle; the drop always lands on the page
        // that's live at release).
        let vp = layout.sections[content::SECTION_APPS].viewport;
        if layout.sections[content::SECTION_APPS].n_pages > 1 {
            let dir = edge_page_dir(vp.x, vp.w, pos.0);
            if edge_page_due(&mut self.grid_drag_page_at, dir) {
                info!("drag page turn: dir {dir}");
                self.page_by(content::SECTION_APPS, dir, true);
            }
            if dir != 0 {
                // Keep frames coming while the dwell clock runs (a
                // stationary pointer at the edge emits no motion).
                self.dirty = true;
            }
        } else {
            self.grid_drag_page_at = None;
        }
        // The resting gap: a reorder rests at the item's own display slot
        // (nothing moves on pickup); an insert has no gap (the grid stays
        // whole) until the pointer asks for a slot.
        let orig_slot = orig.and_then(|o| self.apps_slots.get(o).copied());

        // Off the grid: rest the gap (a reorder at its slot, an insert to
        // none) — you may be reaching for the dock, so the grid closes up.
        // EXCEPT beside the grid in the edge-paging overshoot: the pointer
        // there is actively flipping pages, so it targets the live page's
        // append slot — the SH edge-drop bug was exactly this zone
        // resolving to "outside" (the drop no-op'd and snapped back).
        let Some((d, fx, _fy)) = self.apps_display_cell(layout, pos) else {
            let beside = pos.1 >= vp.y
                && pos.1 <= vp.y + vp.h
                && edge_page_dir(vp.x, vp.w, pos.0) != 0;
            let want = beside.then(|| {
                let cap = self.apps_cap.max(1);
                let page_start = self
                    .scroll.per[content::SECTION_APPS]
                    .page(vp.w.max(1.0))
                    .min(layout.sections[content::SECTION_APPS].n_pages.saturating_sub(1))
                    * cap;
                let append = self
                    .apps_slots
                    .iter()
                    .enumerate()
                    .filter(|&(j, &s)| Some(j) != orig && s >= page_start && s < page_start + cap)
                    .map(|(_, &s)| s + 1)
                    .max()
                    .unwrap_or(page_start);
                // Same-page guard as below: a reorder within the live
                // page must not target one past its last slot.
                match orig_slot {
                    Some(o) if (page_start..page_start + cap).contains(&o) => {
                        append.saturating_sub(1).max(o)
                    }
                    _ => append,
                }
            });
            let want = want.or(orig_slot);
            if self.reorder_slot != want {
                self.reorder_slot = want;
                self.dirty = true;
            }
            return None;
        };
        // The hovered page's append position: one past its last occupied
        // slot (the dragged item's own slot is the hole in hand, not an
        // obstacle). An empty page — the ghost page — appends at its
        // first cell.
        let cap = self.apps_cap.max(1);
        let page_start = (d / cap) * cap;
        let append = self
            .apps_slots
            .iter()
            .enumerate()
            .filter(|&(j, &s)| Some(j) != orig && s >= page_start && s < page_start + cap)
            .map(|(_, &s)| s + 1)
            .max()
            .unwrap_or(page_start);
        // One past the last occupied slot is only landable when the page
        // GAINS an item (insert / cross-page move). A same-page reorder
        // neither grows nor shrinks the page, so its last landable slot
        // is the page's current last — clamping to `append` sent an
        // end-of-page drop one slot too far: onto the next page.
        let last_slot = match orig_slot {
            Some(o) if (page_start..page_start + cap).contains(&o) => {
                append.saturating_sub(1).max(o)
            }
            _ => append,
        };
        // Make room continuously (Launchpad). Continuous row-major position
        // of the pointer (monotonic across row wraps): `d` is the cell, `fx`
        // the fraction across it.
        let p = d as f32 + fx;
        let want = match orig_slot {
            // Reorder: symmetric make-room. The gap rests at the dragged
            // item's home slot and moves only once the pointer passes a
            // *neighbour's* centre — a full cell of travel either way — so a
            // neighbour holds still until you cross its middle, on both sides
            // equally, instead of the far side snapping into the freed hole
            // the instant you nudge toward it. (A neighbour that holds still
            // is also what lets you drop onto it to make a box.)
            Some(o) => {
                let rel = p - (o as f32 + 0.5);
                let steps = if rel >= 1.0 {
                    rel.floor() as i64
                } else if rel <= -1.0 {
                    -((-rel).floor() as i64)
                } else {
                    0
                };
                (o as i64 + steps).clamp(page_start as i64, last_slot as i64) as usize
            }
            // Insert (dock / package / webapp drag): no hole to close, so
            // icon centres are the seam boundaries directly.
            None => (d + usize::from(fx >= 0.5)).min(append),
        };
        if self.reorder_slot != Some(want) {
            self.reorder_slot = Some(want);
            self.dirty = true;
        }
        None
    }

    /// Pin `id` onto the dock at the visible insert position `insert`
    /// (`drag_dock_insert` space): everything left of the drop keeps its
    /// exact place, so the usage-filled slots there are first materialized
    /// into explicit pins (pin_at's index is pins-relative). `exclude` is
    /// the dragged entry (skipped, and — for a dock-origin drag — its
    /// removal from the compact coordinates shifts the raw slot). Returns
    /// the pins-relative slot `id` was pinned at.
    pub(crate) fn pin_dropped_on_dock(
        &mut self,
        id: &str,
        insert: usize,
        exclude: Option<usize>,
        from_dock: bool,
    ) -> usize {
        let slot = if from_dock {
            let origin = exclude.and_then(|x| self.dock_order.iter().position(|&e| e == x));
            insert - usize::from(origin.is_some_and(|o| o < insert))
        } else {
            insert
        };
        let prefix: Vec<String> = self
            .dock_order
            .iter()
            .filter(|&&e| Some(e) != exclude)
            .take(slot)
            .map(|&e| self.entries[e].id.clone())
            .collect();
        for (k, pid) in prefix.iter().enumerate() {
            self.pins.pin_at(pid, k);
        }
        self.pins.pin_at(id, slot);
        slot
    }

    /// Resolve a grid-origin drop; true when it was a grid gesture
    /// (fold / reorder / leave-box), false to fall through to pinning.
    pub(crate) fn handle_grid_drop(&mut self, entry_idx: usize, id: &str, pos: (f32, f32)) -> bool {
        let layout = self.current_layout();
        let kind = self.kinds.get(entry_idx).copied();
        let visible = &self.search.visible[content::SECTION_APPS];
        // A grid app/box knows its own cell; a dock app dragged in to
        // unpin has none (`orig` == None) and lands as a fresh insert.
        let orig = visible.iter().position(|&v| v == entry_idx);
        let vp = layout.sections[content::SECTION_APPS].viewport;
        // The edge-paging overshoot beside the grid counts as ON the grid:
        // a release there lands on the live page (the pointer was parked
        // there flipping pages — treating it as "outside" silently no-op'd
        // the whole drag-to-new-page gesture; SH edge-drop bug).
        let beside = pos.1 >= vp.y
            && pos.1 <= vp.y + vp.h
            && edge_page_dir(vp.x, vp.w, pos.0) != 0;
        if !vp.contains(pos) && !beside {
            // Dropped outside the grid (a dock app snaps back and stays
            // pinned; a grid member dragged out just returns): no-op.
            // DIAG (end-of-grid drop bug, 2026-08-31): say exactly where.
            info!(
                "grid drop NO-OP: pos ({:.0},{:.0}) outside apps vp ({:.0},{:.0} {:.0}x{:.0})",
                pos.0, pos.1, vp.x, vp.y, vp.w, vp.h
            );
            return false;
        }
        // A dock app that lands in the grid unpins as it does.
        if orig.is_none() {
            self.pins.unpin(id);
            // If it was a just-installed app riding the dock's "fresh install"
            // zone, retire that notify — otherwise the grid keeps hiding it
            // (it's still "notified") and `recompute_dock_order` snaps it back
            // onto the dock. This is the third exit from the zone, alongside
            // opening it and pinning it left of the divider.
            self.install_notify.retain(|n| n.id.as_str() != id);
        }
        let orig_slot = orig.and_then(|o| self.apps_slots.get(o).copied());
        let slot = self
            .reorder_slot
            .unwrap_or(orig_slot.unwrap_or(self.apps_span));
        // DIAG (end-of-grid drop bug, 2026-08-31): the resolved landing.
        info!(
            "grid drop: slot {slot} (reorder_slot {:?}, orig_slot {:?}, pos ({:.0},{:.0}))",
            self.reorder_slot, orig_slot, pos.0, pos.1
        );
        // Fold wins when the pointer sits on a foldable item's centre —
        // the exact same detection and box create/join the Install drops
        // use, so every placement shares one path. (Boxes don't nest, so
        // only a dragged App folds.)
        if kind == Some(apps::EntryKind::App) {
            if let Some(target) = self.grid_fold_at(pos, entry_idx) {
                // Dropped onto the Recycle Bin (if it's been moved to the
                // grid): uninstall instead of folding into a box.
                if self
                    .entries
                    .get(target)
                    .is_some_and(|e| groups::is_trash(&e.id))
                {
                    self.uninstall_app(id);
                } else {
                    self.fold_box_for(target, id);
                }
                self.refilter();
                return true;
            }
        }
        // Otherwise the drop lands in the gap, wherever it is now.
        if let Some(g) = self.app_group {
            // Legacy in-open-box path: gap slots are member positions.
            let n_members = self.groups.groups()[g].members.len();
            let full_before = (slot + usize::from(orig.is_some_and(|o| slot >= o))).min(n_members);
            self.groups.move_member(g, id, full_before);
        } else {
            self.place_in_grid_at_slot(id, entry_idx, slot);
        }
        self.refilter();
        true
    }

    /// Place `id` in the Apps-grid order at display `slot`, by the shared
    /// drop rule: before the first grid item at-or-past the gap on its
    /// page, or — a page's empty tail, or the ghost page — appended to
    /// that page (the ghost drop creates it: the drag-to-new-page
    /// gesture). `dragged_idx` is excluded as an anchor candidate so an
    /// item never anchors on itself. Caller refilters.
    fn place_in_grid_at_slot(&mut self, id: &str, dragged_idx: usize, slot: usize) {
        let cap = self.apps_cap.max(1);
        // Resolve the drop against the *shifted* positions the make-room
        // previews — the dragged item's hole closed, the gap opened — so a
        // rightward move lands exactly where it showed instead of anchoring
        // on the neighbour that already slid into the hole (which read as a
        // swap on screen but no-op'd on drop, snapping both back).
        let orig_slot = self.search.visible[content::SECTION_APPS]
            .iter()
            .position(|&e| e == dragged_idx)
            .and_then(|o| self.apps_slots.get(o).copied());
        let items = self.search.visible[content::SECTION_APPS]
            .iter()
            .enumerate()
            .filter(|&(_, &e)| e != dragged_idx)
            .filter_map(|(j, &e)| {
                self.apps_slots
                    .get(j)
                    .map(|&s| (pages::shifted_slot(s, orig_slot, slot, cap), e))
            });
        match pages::drop_anchor(items, slot, cap) {
            Some(e) => {
                let a = self.entries[e].id.clone();
                self.order.move_before(id, &a);
            }
            None => {
                let sp = self
                    .apps_page_map
                    .get(slot / cap)
                    .copied()
                    .unwrap_or_else(|| self.order.pages().len());
                self.order.move_to_page_end(id, sp);
            }
        }
    }

    /// The section that would accept the dragged entry if dropped at
    /// `pos` (drop-target highlight): Apps installs a package, Install
    /// uninstalls an app.
    pub(crate) fn drag_drop_section(
        &self,
        layout: &content::Layout,
        pos: (f32, f32),
        entry_idx: usize,
    ) -> Option<usize> {
        // Out in the margin the drop is a "try it" launch, not an install
        // — don't highlight the grid. `section_at` is Y-only, so at grid
        // altitude it still reports SECTION_APPS without this guard.
        if self.outside_card(layout, pos) {
            return None;
        }
        let section = content::section_at(layout, pos)?;
        match self.kinds.get(entry_idx) {
            Some(apps::EntryKind::Package) if section == content::SECTION_APPS => Some(section),
            // Only apps waverunner installed can be uninstalled here — a
            // managed package (in the home-manager list) or an installed
            // catalog webapp. Base / system apps don't offer the target at
            // all (a drop just snaps back).
            Some(apps::EntryKind::App)
                if section == content::SECTION_INSTALL
                    && self.entries.get(entry_idx).is_some_and(|e| {
                        self.removable_ids.contains(&e.id)
                            || self.installed_webapp_slug(&e.id).is_some()
                    }) =>
            {
                Some(section)
            }
            _ => None,
        }
    }
}

impl App {
    /// The slug of a *catalog* webapp id (materialized but not installed),
    /// or `None` for a non-webapp or already-installed id.
    fn catalog_webapp_slug(&self, id: &str) -> Option<String> {
        crate::webapps::slug_of_id(id)
            .filter(|s| !self.managed_webapps.contains(s))
            .map(str::to_string)
    }

    /// The slug of an *installed* webapp id, or `None` otherwise.
    fn installed_webapp_slug(&self, id: &str) -> Option<String> {
        crate::webapps::slug_of_id(id)
            .filter(|s| self.managed_webapps.contains(s))
            .map(str::to_string)
    }

    /// Install a catalog webapp: record it so it moves from the Install
    /// section onto the grid, land it in the grid order at the drop slot
    /// (the make-room gap, like a package's captured anchor — without
    /// this it teleports to its long-ago-materialized end-of-grid slot),
    /// then refilter to relocate its tile. `dragged_idx` is the webapp's
    /// Install-section entry (excluded as an anchor candidate).
    fn install_webapp(&mut self, slug: &str, dragged_idx: usize) {
        info!("installing webapp {slug}");
        self.managed_webapps.add(slug);
        let id = crate::webapps::id_for_slug(slug);
        let slot = self.reorder_slot.unwrap_or(self.apps_span);
        self.place_in_grid_at_slot(&id, dragged_idx, slot);
        // A webapp installs instantly, but play the same progress ring for a
        // beat (see `PendingWebapp`) so it lands with the same feel as a
        // package; `draw` fills and then retires the ring.
        self.pending_webapps.push(crate::install::PendingWebapp {
            id,
            started: Instant::now(),
            completed_at: None,
        });
        self.refilter();
        self.schedule_frame();
    }

    /// Install a catalog webapp straight onto the dock, pinned at the slot
    /// it was dropped on (the dock counterpart of [`Self::install_webapp`]).
    fn install_webapp_on_dock(&mut self, slug: &str, insert: usize) {
        info!("installing webapp {slug} onto the dock");
        self.managed_webapps.add(slug);
        let id = crate::webapps::id_for_slug(slug);
        self.pin_dropped_on_dock(&id, insert, None, false);
        // Play the same ring-fill + shine flourish a grid install shows, over
        // the pinned dock slot (the dock now draws both — see `content`). Without
        // this a drag-straight-to-dock webapp appeared with no feedback at all.
        self.pending_webapps.push(crate::install::PendingWebapp {
            id,
            started: Instant::now(),
            completed_at: None,
        });
        self.refilter();
        self.schedule_frame();
    }

    /// The grid app or box under `pos` that a fold would target — dropping
    /// on an app's centre boxes the two, on a box joins it. `None` off a
    /// foldable cell. Shared by grid-origin drops (via `handle_grid_drop`)
    /// and install drops, so both fold the same way.
    pub(crate) fn grid_fold_at(&self, pos: (f32, f32), dragged_idx: usize) -> Option<usize> {
        if self.app_group.is_some() {
            return None; // an open box places members its own way
        }
        let layout = self.current_layout();
        let (d, fx, fy) = self.apps_display_cell(&layout, pos)?;
        // Fold only on the centre of a real cell that ISN'T the make-room
        // gap — a drop in the gap (where the ghost already sits) reorders.
        // Without this a drag would fold onto nearly every icon it passed.
        let slot = self.reorder_slot?;
        if d == slot || !FOLD_BAND.contains(&fx) || !(0.08..0.92).contains(&fy) {
            return None;
        }
        let j = self.apps_slots.iter().position(|&s| s == d)?;
        let target = *self.search.visible[content::SECTION_APPS].get(j)?;
        if target == dragged_idx {
            return None; // never fold an item onto itself
        }
        matches!(
            self.kinds.get(target),
            Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
        )
        .then_some(target)
    }

    /// Resolve the box a fold onto grid `target_idx` lands in: an app makes
    /// a new box (placed at the app's grid slot); a box is joined. Returns
    /// the group id, or `None` if the target turned out not to be foldable.
    fn fold_box_for(&mut self, target_idx: usize, member_id: &str) -> Option<String> {
        let target_id = self.entries.get(target_idx)?.id.clone();
        match self.kinds.get(target_idx) {
            Some(apps::EntryKind::Group) => {
                let gid = target_id.strip_prefix("group:")?.to_owned();
                let g = self.groups.index_by_id(&gid)?;
                self.groups.add(g, member_id);
                Some(gid)
            }
            Some(apps::EntryKind::App) => {
                let gid = self.groups.create(&target_id, member_id);
                self.order
                    .insert_before(&format!("group:{gid}"), &target_id);
                Some(gid)
            }
            _ => None,
        }
    }

    /// Drag-to-install that lands in a box: fold `attr` onto grid `target`
    /// (new box from an app, or join a box), then install with that box as
    /// the pending tile's destination so the finished app replaces it there.
    fn start_pending_install_boxed(
        &mut self,
        attr: &str,
        name: String,
        version: String,
        target: usize,
    ) {
        match self.fold_box_for(target, attr) {
            Some(box_id) => self.start_pending_install(attr, name, version, None, Some(box_id)),
            // Not foldable after all — a plain grid install at the drop slot.
            None => self.start_pending_install(attr, name, version, None, None),
        }
    }

    /// Install a catalog webapp folded into a box (create/join), the box
    /// counterpart of [`Self::install_webapp`] — instant, no pending tile.
    fn install_webapp_boxed(&mut self, slug: &str, target: usize, dragged_idx: usize) {
        info!("installing webapp {slug} into a box");
        self.managed_webapps.add(slug);
        let id = crate::webapps::id_for_slug(slug);
        if self.fold_box_for(target, &id).is_none() {
            // Target wasn't foldable — fall back to a plain grid placement.
            let slot = self.reorder_slot.unwrap_or(self.apps_span);
            self.place_in_grid_at_slot(&id, dragged_idx, slot);
        }
        self.refilter();
        self.schedule_frame();
    }

    /// Uninstall a webapp: drop the record so it returns to the catalog
    /// (its launcher file stays on disk).
    fn uninstall_webapp(&mut self, slug: &str) {
        info!("uninstalling webapp {slug}");
        let id = crate::webapps::id_for_slug(slug);
        // Close it first if it's open, so uninstalling a running webapp closes
        // its window instead of leaving it up as an orphaned catalog entry.
        self.kill_app_windows(&id);
        self.managed_webapps.remove(slug);
        // Drop any dock pin. A webapp's `.desktop` stays materialized after
        // uninstall (it returns to the catalog), so a lingering pin would keep
        // matching that App entry in `recompute_dock_order` — the icon would
        // stick to the dock as an uninstalled catalog entry with no unpin path
        // (catalog-webapp drags only try/install). This is the webapp analogue
        // of the package uninstall's `pins.unpin` in `nix::Event::Done`.
        self.pins.unpin(&id);
        self.refilter(); // refilter recomputes the dock order
        self.schedule_frame();
    }

    /// "Try it": launch a catalog webapp's Chrome app-mode command without
    /// installing it (its window shows in the dock unpinned while open).
    fn launch_webapp(&mut self, idx: usize) {
        let Some(entry) = self.entries.get(idx) else {
            return;
        };
        let exec = entry.exec.clone();
        if let Err(e) = crate::launch::launch(&exec, false, &self.config.launch.terminal) {
            tracing::error!("try-launch webapp failed: {e}");
        }
    }
}

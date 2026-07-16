//! Grid and dock drag & drop: the make-room gap and its dwell, edge
//! paging while a drag holds at a side, fold targets (app-onto-app boxes
//! and dock folders), and the drop resolution for every drag origin.

use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::{apps, content, nix, pages};
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

/// How long the pointer must linger over a new grid slot before the
/// make-room gap moves there. Folding is immediate; reordering is
/// deliberate — that split keeps side-neighbor folds reachable.
pub(crate) const REORDER_DWELL: Duration = Duration::from_millis(180);

/// Horizontal band within a cell (fraction of its width) where hovering
/// an app rings a fold target and a drop makes/joins a box. Wide enough
/// that folding onto a side-by-side neighbour is easy — only the narrow
/// edges (aim at the gap between icons) make room to reorder instead.
pub(crate) const FOLD_BAND: std::ops::Range<f32> = 0.225..0.775;

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

    /// Finish a drag. Apps pin at the dock slot they were dropped on
    /// (dock-origin drags dropped elsewhere unpin), a profile app
    /// dropped on the Install section uninstalls, and a package dropped
    /// on the Apps section (or the dock) installs. `released` is false
    /// when the drag ended by the pointer leaving the surface — that
    /// path never installs or uninstalls anything.
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
                } else if section == Some(content::SECTION_APPS) {
                    // Fresh package dropped into the grid: place it as a
                    // tile at the drop slot and install it in place.
                    self.start_pending_install(&id, pkg_name, pkg_version);
                } else if insert.is_some() && !self.busy_ids.contains(&id) {
                    // Dropped on the dock band: plain install (no tile),
                    // the app appears wherever its grid order puts it.
                    self.start_managed_install(&id, Vec::new());
                }
            }
            Some(apps::EntryKind::App)
                if section == Some(content::SECTION_INSTALL)
                    && self.removable_ids.contains(&id)
                    && !self.busy_ids.contains(&id) =>
            {
                // Uninstall — gated to managed apps, so this only runs for
                // something the launcher installed (non-removable apps
                // fall through and snap back). Remove the attr from the
                // list, then switch; the busy id is the app cell.
                if let Some(attr) = self.managed.attr_for_app(&id) {
                    info!("uninstalling {id} (attr {attr})");
                    self.managed.remove(&attr);
                    // The install pinned it onto the dock; drop that pin so
                    // it doesn't linger once the app is gone.
                    self.pins.unpin(&id);
                    self.recompute_removable();
                    self.busy_ids.insert(id.clone());
                    self.nix.request(nix::Request::Switch { id });
                } else {
                    debug!("{id} is not waverunner-managed; cannot uninstall");
                }
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
                    && self.search.query.is_empty()
                    && self.handle_grid_drop(drag.entry_idx, &id, drag.pos);
                // Dropped centered on a dock icon: join/create a folder on
                // the dock (an app onto a folder joins; onto an app boxes).
                let dock_folded = !boxed
                    && released
                    && self.search.query.is_empty()
                    && kind == Some(apps::EntryKind::App)
                    && self.handle_dock_fold(drag.entry_idx, &id, drag.pos);
                if !boxed && !dock_folded {
                    // Dropped on the dock pins the item (app or box) there.
                    if let Some(slot) = insert {
                        // Dock reorder. Drop in the visible gap: the
                        // dock parts in compact coordinates (origin
                        // removed), so translate the raw slot.
                        let slot = if drag.from_dock {
                            let origin = self.dock_order.iter().position(|&e| e == drag.entry_idx);
                            slot - usize::from(origin.is_some_and(|o| o < slot))
                        } else {
                            slot
                        };
                        // Everything left of the drop keeps its exact
                        // place: usage-filled slots there become
                        // explicit pins first (pin_at's index is
                        // pins-relative).
                        let prefix: Vec<String> = self
                            .dock_order
                            .iter()
                            .filter(|&&e| e != drag.entry_idx)
                            .take(slot)
                            .map(|&e| self.entries[e].id.clone())
                            .collect();
                        for (k, pid) in prefix.iter().enumerate() {
                            self.pins.pin_at(pid, k);
                        }
                        self.pins.pin_at(&id, slot);
                    }
                    // insert == None: dropped outside the dock band —
                    // no unpin, the icon just returns to the dock.
                }
            }
        }
        self.reorder_slot = None;
        self.reorder_dwell = None;
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
        let ax = (pos.0 - sec.viewport.x + sec.scroll).max(0.0);
        let page = ((ax / page_w).floor() as usize) % sec.n_pages.max(1);
        let fx = (ax.rem_euclid(page_w)) / content::GRID_CELL_W;
        let fy = (pos.1 - sec.viewport.y) / content::GRID_CELL_H;
        let col = (fx.floor() as usize).min(sec.cols.saturating_sub(1));
        let row = (fy.floor() as usize).min(sec.rows.saturating_sub(1));
        let d = page * sec.cols * sec.rows + row * sec.cols + col;
        Some((d, fx.fract(), fy.fract().clamp(0.0, 1.0)))
    }

    /// Track the drag's grid target in display space. The make-room
    /// gap starts at the pickup slot (nothing moves on pickup); it
    /// only moves after the pointer *lingers* over a new slot
    /// ([`REORDER_DWELL`]) — that dwell is what makes folding onto a
    /// side neighbor possible at all: icons no longer dive out of the
    /// way the moment you approach them. Hovering an item rings it as
    /// a fold target immediately. Returns the fold target, if any;
    /// the gap itself lives in `self.reorder_slot`.
    pub(crate) fn update_grid_target(
        &mut self,
        layout: &content::Layout,
    ) -> Option<(usize, usize)> {
        let Some(drag) = self.gesture.dragging.as_ref() else {
            self.reorder_slot = None;
            self.reorder_dwell = None;
            self.grid_drag_page_at = None;
            return None;
        };
        let kind = self.kinds.get(drag.entry_idx).copied();
        let visible = &self.search.visible[content::SECTION_APPS];
        let (len, pos) = (visible.len(), drag.pos);
        let orig = visible.iter().position(|&v| v == drag.entry_idx);
        // Three ways to target the grid: a grid-origin app/box reordering
        // itself, a dock-origin app dragged in to unpin it, or a package
        // dragged up from Install to be placed as it installs. The latter
        // two own no cell, so the gap is a brand-new slot (nothing to
        // vacate) and may land one past the end (append).
        let inserting = drag.from_dock || kind == Some(apps::EntryKind::Package);
        let grid_drag = self.search.query.is_empty()
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
            self.reorder_dwell = None;
            self.grid_drag_page_at = None;
            return None;
        }
        // Edge-paging: holding the drag in the grid's edge band turns
        // the page every DRAG_PAGE_COOLDOWN, carrying the icon across
        // pages (shared band + dwell with the open-box drag).
        let vp = layout.sections[content::SECTION_APPS].viewport;
        if layout.sections[content::SECTION_APPS].n_pages > 1 {
            let dir = edge_page_dir(vp.x, vp.w, pos.0);
            if edge_page_due(&mut self.grid_drag_page_at, dir) {
                info!("drag page turn: dir {dir}");
                self.page_by(content::SECTION_APPS, dir, false);
            }
            if dir != 0 {
                // Keep frames coming while the dwell clock runs (a
                // stationary pointer at the edge emits no motion).
                self.dirty = true;
            }
        } else {
            self.grid_drag_page_at = None;
        }
        // The gap rests at the app's own display slot (reorder — nothing
        // moves on pickup) or past everything (insert — the grid stays
        // whole until the pointer asks for a slot).
        let orig_slot = orig.and_then(|o| self.apps_slots.get(o).copied());
        let slot = *self
            .reorder_slot
            .get_or_insert(orig_slot.unwrap_or(self.apps_span));

        // Off the grid: a reorder leaves the gap where it was (you may
        // be reaching for the dock); an insert closes the grid back up.
        let Some((d, fx, fy)) = self.apps_display_cell(layout, pos) else {
            if inserting {
                self.reorder_slot = None;
            }
            self.reorder_dwell = None;
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
        if d == slot {
            self.reorder_dwell = None;
            return None; // hovering the gap: stable
        }
        // The item at the hovered slot rings a fold target immediately
        // (apps only; boxes never fold or nest). Empty slots don't fold.
        if kind == Some(apps::EntryKind::App)
            && FOLD_BAND.contains(&fx)
            && (0.08..0.92).contains(&fy)
        {
            let full = self
                .apps_slots
                .iter()
                .position(|&s| s == d)
                .filter(|&j| Some(j) != orig);
            let foldable = full.and_then(|j| self.search.visible[content::SECTION_APPS].get(j));
            if let (Some(full), Some(&t)) = (full, foldable) {
                let ok = match self.kinds.get(t) {
                    Some(apps::EntryKind::Group) => true,
                    Some(apps::EntryKind::App) => self.app_group.is_none(),
                    _ => false,
                };
                if ok {
                    self.reorder_dwell = None;
                    return Some((content::SECTION_APPS, full));
                }
            }
        }
        // Reordering: move the gap only after a dwell. A hover past a
        // page's items snaps to its append slot (tail gaps and the ghost
        // page land there). Only a dock insert gets the right-edge nudge
        // (so it can append past the last icon); a reorder moves the gap
        // straight to the hovered cell. The old unconditional nudge could
        // leap the gap over a neighbour, sliding two icons for one step.
        let want = if inserting && fx >= 0.85 {
            (d + 1).min(append)
        } else {
            d.min(append)
        };
        if want == slot {
            self.reorder_dwell = None;
            return None;
        }
        match self.reorder_dwell {
            Some((w, since)) if w == want => {
                if since.elapsed() >= REORDER_DWELL {
                    self.reorder_slot = Some(want);
                    self.reorder_dwell = None;
                }
                // Keep frames coming while the dwell clock runs.
                self.dirty = true;
            }
            _ => {
                self.reorder_dwell = Some((want, Instant::now()));
                self.dirty = true;
            }
        }
        None
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
        let inside = layout.sections[content::SECTION_APPS]
            .viewport
            .contains(pos);
        if !inside {
            // Dropped outside the grid (a dock app snaps back and stays
            // pinned; a grid member dragged out just returns): no-op.
            return false;
        }
        // A dock app that lands in the grid unpins as it does.
        if orig.is_none() {
            self.pins.unpin(id);
        }
        let orig_slot = orig.and_then(|o| self.apps_slots.get(o).copied());
        let slot = self
            .reorder_slot
            .unwrap_or(orig_slot.unwrap_or(self.apps_span));
        // Fold wins when the pointer sits on a foldable item's center.
        if kind == Some(apps::EntryKind::App) {
            if let Some((d, fx, _)) = self.apps_display_cell(&layout, pos) {
                if d != slot && FOLD_BAND.contains(&fx) {
                    // The item occupying the hovered display slot.
                    let full = self
                        .apps_slots
                        .iter()
                        .position(|&s| s == d)
                        .filter(|&j| Some(j) != orig);
                    if let Some(&target_idx) =
                        full.and_then(|j| self.search.visible[content::SECTION_APPS].get(j))
                    {
                        let target_id = self.entries[target_idx].id.clone();
                        match self.kinds.get(target_idx) {
                            Some(apps::EntryKind::Group) => {
                                if let Some(g) = target_id
                                    .strip_prefix("group:")
                                    .and_then(|gid| self.groups.index_by_id(gid))
                                {
                                    self.groups.add(g, id);
                                    self.refilter();
                                    return true;
                                }
                            }
                            Some(apps::EntryKind::App) if self.app_group.is_none() => {
                                // The new box takes the target's grid
                                // position.
                                let box_id = self.groups.create(&target_id, id);
                                self.order
                                    .insert_before(&format!("group:{box_id}"), &target_id);
                                self.refilter();
                                return true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // Otherwise the drop lands in the gap, wherever it is now.
        if let Some(g) = self.app_group {
            // Legacy in-open-box path: gap slots are member positions.
            let n_members = self.groups.groups()[g].members.len();
            let full_before = (slot + usize::from(orig.is_some_and(|o| slot >= o))).min(n_members);
            self.groups.move_member(g, id, full_before);
        } else {
            // Resolve the gap via the shared drop rule: before the item
            // at-or-past the gap on its page, or — a page's empty tail,
            // or the ghost page — append to that page (the ghost drop
            // creates it: the drag-to-new-page gesture).
            let cap = self.apps_cap.max(1);
            let items = self.search.visible[content::SECTION_APPS]
                .iter()
                .enumerate()
                .filter(|&(_, &e)| e != entry_idx)
                .filter_map(|(j, &e)| self.apps_slots.get(j).map(|&s| (s, e)));
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
        self.refilter();
        true
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
            // Only apps waverunner installed (i.e. in the managed
            // home-manager list) can be uninstalled here — base / system
            // apps don't offer the target at all (a drop just snaps back).
            Some(apps::EntryKind::App)
                if section == content::SECTION_INSTALL
                    && self
                        .entries
                        .get(entry_idx)
                        .is_some_and(|e| self.removable_ids.contains(&e.id)) =>
            {
                Some(section)
            }
            _ => None,
        }
    }
}

//! The open box (magnified app group) — opening/closing, the in-box
//! member reorder drag with its edge paging and ghost page, pulling a
//! member out into the loose grid, and the dock stack variants (a group
//! folder, or a pinned directory's content stack).

use tracing::info;
use waverunner_core::index::AppEntry;

use crate::dragging::{edge_page_dir, edge_page_due};
use crate::state::Target;
use crate::{apps, content, groups, pages};
use crate::{App, DragState};

/// A pinned directory's open dock stack: the same magnified box a dock
/// folder uses, fed by the directory's listing instead of a group. The
/// members are file paths (ids of the transient listing entries the
/// refilter rebuilds while the stack is open); clicking one opens it.
pub(crate) struct DirStack {
    /// The pinned dock entry's id (the stack's grow anchor) — a path for
    /// pins dragged from a listing, `folder-<name>` for home-strip pins.
    pub(crate) id: String,
    /// The directory whose contents the stack shows.
    pub(crate) path: String,
    /// The listing, paged 3×3 like group members.
    pub(crate) members: pages::PagedList,
    /// Opened with the card up: the box anchors into the grid (like a
    /// dock folder does then) instead of floating above the dock.
    pub(crate) in_grid: bool,
}

impl App {
    /// Build one transient Apps-grid cell per group (id
    /// `group:<stable-id>`, kind Group) and record the member texture
    /// layers for the 2×2 mini preview. Returns the cells' entry
    /// indices, group order.
    pub(crate) fn group_cells(&mut self) -> Vec<usize> {
        // Snapshot first: labels and member layers read groups+entries
        // while pushing mutates the entry arrays.
        let snapshot: Vec<(String, String, [Option<u32>; 4])> = (0..self.groups.groups().len())
            .map(|g| {
                let label = self.groups.label(g, |id| {
                    self.entries
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| e.name.clone())
                });
                let mut minis = [None; 4];
                for (k, member) in self.groups.groups()[g].members.iter().take(4).enumerate() {
                    minis[k] = self
                        .entries
                        .iter()
                        .position(|e| &e.id == member)
                        .and_then(|idx| self.icon_layers.get(idx).copied());
                }
                (
                    format!("group:{}", self.groups.groups()[g].id),
                    label,
                    minis,
                )
            })
            .collect();
        snapshot
            .into_iter()
            .map(|(gid, label, minis)| {
                let entry = AppEntry {
                    id: gid,
                    name: label,
                    description: None,
                    exec: String::new(),
                    icon: None,
                    startup_wm_class: None,
                    needs_terminal: false,
                    path: None,
                };
                let idx = self.push_transient(entry, apps::EntryKind::Group, false, 0);
                self.group_minis.push((idx, minis));
                idx
            })
            .collect()
    }

    /// Open a box: the magnified box grows out of the tile's cell. The
    /// origin is snapped to the tile's cell *center* (not the click point)
    /// so the box collapses back exactly onto the resting tile — no jump.
    pub(crate) fn open_group(&mut self, g: usize) {
        let layout = self.current_layout();
        self.group_origin = self.pointer_pos.map(|pos| {
            match self.apps_display_cell(&layout, pos) {
                // Snap to the tile's center: cell center horizontally, and
                // the icon center vertically (the tile sits high in the cell).
                Some((_, fx, fy)) => {
                    let cell_top = pos.1 - fy * content::GRID_CELL_H;
                    (
                        pos.0 - (fx - 0.5) * content::GRID_CELL_W,
                        cell_top + content::GRID_ICON_TOP + content::GRID_ICON / 2.0,
                    )
                }
                None => pos,
            }
        });
        self.app_group = Some(g);
        self.reset_stack_view();
    }

    /// The shared tail of every stack open (grid box, dock folder,
    /// directory stack): rewind the grow animation and box paging state,
    /// refresh the input region, and rebuild the frame's inputs.
    fn reset_stack_view(&mut self) {
        self.group_anim = 0.0;
        self.group_anim_target = 1.0;
        self.box_page = 0;
        self.box_pager.reset();
        self.box_slide.clear();
        self.sync_input_region();
        self.refilter();
    }

    /// Close the open box (grid box, dock stack, or directory stack): it
    /// shrinks back into its tile; the state clears once the animation
    /// lands (see `draw`).
    pub(crate) fn close_group(&mut self) {
        if !self.stack_open() {
            return;
        }
        if self.group_origin.is_none() {
            self.group_origin = self.pointer_pos;
        }
        self.group_anim_target = 0.0;
        self.schedule_frame();
    }

    /// The group whose magnified box is open — in the grid (`app_group`) or
    /// as a dock folder stack (`dock_stack`). They're mutually exclusive and
    /// drive the same box overlay; only the box's resting position differs.
    pub(crate) fn open_box_group(&self) -> Option<usize> {
        self.app_group.or(self.dock_stack)
    }

    /// Whether any stack is open: a group box (grid or dock) or a pinned
    /// directory's content stack.
    pub(crate) fn stack_open(&self) -> bool {
        self.open_box_group().is_some() || self.dir_stack.is_some()
    }

    /// The open stack's member ids — a group's members, or the listing of
    /// an open pinned-directory stack. One paged list feeds the box
    /// overlay either way.
    pub(crate) fn stack_members(&self) -> Option<&pages::PagedList> {
        if let Some(ds) = &self.dir_stack {
            return Some(&ds.members);
        }
        self.open_box_group()
            .and_then(|g| self.groups.groups().get(g))
            .map(|grp| &grp.members)
    }

    /// Center of the dock icon whose entry has `id` (a stack's grow origin).
    pub(crate) fn dock_slot_center(&self, id: &str) -> Option<(f32, f32)> {
        let layout = self.current_layout();
        self.dock_order
            .iter()
            .position(|&e| self.entries.get(e).is_some_and(|x| x.id == id))
            .and_then(|slot| layout.dock_slots.get(slot))
            .map(|r| (r.x + r.w / 2.0, r.y + r.h / 2.0))
    }

    /// Center of a box's dock folder icon (its grow origin), by the box id.
    pub(crate) fn dock_folder_center(&self, g: usize) -> Option<(f32, f32)> {
        let gid = format!("group:{}", self.groups.groups()[g].id);
        self.dock_slot_center(&gid)
    }

    /// Open a pinned directory's content stack — the same magnified box a
    /// dock folder uses, fed by the directory listing (built by the
    /// refilter), with the same dual anchoring: with the card open it
    /// grows out of the dock icon into the grid; docked, it floats above
    /// the icon. `id` is the pinned entry's id (its dock anchor); `path`
    /// the directory it stands for.
    pub(crate) fn open_dir_stack(&mut self, id: String, path: String) {
        info!("dir stack: {path}");
        self.group_origin = self.dock_slot_center(&id);
        self.app_group = None;
        self.dock_stack = None;
        self.dir_stack = Some(DirStack {
            id,
            path,
            members: pages::PagedList::default(),
            in_grid: self.ui.target() == Target::Open,
        });
        self.reset_stack_view();
    }

    /// Open a dock folder. With the grid up, the box grows out of its dock
    /// icon into the grid (a normal grid box); docked, it opens as a stack
    /// above the icon. Either way it's the same magnified box.
    pub(crate) fn open_dock_folder(&mut self, g: usize) {
        self.group_origin = self.dock_folder_center(g);
        if self.ui.target() == Target::Open {
            self.app_group = Some(g);
        } else {
            self.dock_stack = Some(g);
        }
        self.reset_stack_view();
    }

    /// Display name of the open group, for the Apps section title.
    pub(crate) fn apps_group_name(&self) -> String {
        let Some(g) = self.app_group else {
            return String::new();
        };
        self.groups.label(g, |id| {
            self.entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.name.clone())
        })
    }

    /// Whether the open box sits on the left half of the Apps grid (which
    /// side it's on decides the member→slot layout).
    pub(crate) fn box_is_left(&self) -> bool {
        let vp = self.current_layout().sections[content::SECTION_APPS].viewport;
        self.group_origin
            .is_none_or(|(ox, _)| ox < vp.x + vp.w / 2.0)
    }

    /// The open box member id shown in 3×3 slot `k` on the current page, if
    /// that slot is filled (maps slot → member via the side layout + page).
    pub(crate) fn open_box_member_id(&self, k: usize) -> Option<String> {
        let local = content::open_box_slot_member(self.box_is_left(), k);
        self.stack_members()?
            .pages()
            .get(self.box_page)?
            .get(local)
            .cloned()
    }

    /// Entry index of the member in the open box's slot `k`, if filled.
    pub(crate) fn open_box_member_idx(&self, k: usize) -> Option<usize> {
        let id = self.open_box_member_id(k)?;
        self.entries.iter().position(|e| e.id == id)
    }

    /// Begin dragging the open box's `k`-th member out of the box: the app
    /// leaves the box (which closes, and dissolves if left with fewer than
    /// two members) and rides the drag into the loose grid — from there
    /// the normal make-room/drop path organizes it onto the grid or dock.
    /// Start reordering a box member: it lifts out as a ghost but the box
    /// stays open, so it can be moved between slots and pages — or dragged
    /// out of the box to pull it into the grid.
    pub(crate) fn begin_box_drag(&mut self, k: usize, pos: (f32, f32)) {
        // A directory stack mirrors the filesystem: its listing has no
        // order or membership of ours to edit, so members don't drag.
        if self.dir_stack.is_some() {
            return;
        }
        let Some(member_id) = self.open_box_member_id(k) else {
            return;
        };
        if self.busy_ids.contains(&member_id) {
            return;
        }
        let Some(entry_idx) = self.entries.iter().position(|e| e.id == member_id) else {
            return;
        };
        self.box_drag = Some((entry_idx, pos));
        self.box_drag_page_at = None;
        self.gesture.pressed = None;
        self.schedule_frame();
    }

    /// Move the in-box drag: page at the edges, or — for a grid box dragged
    /// out of the box — convert to a pull-out into the loose grid.
    pub(crate) fn update_box_drag(&mut self, pos: (f32, f32)) {
        let Some((entry_idx, _)) = self.box_drag else {
            return;
        };
        let Some(box_rect) = self.current_layout().open_box else {
            return;
        };
        // Pull the member out when the drag exits a clear margin past the
        // box — half a cell beyond, so the paging band (which is forgiving:
        // anywhere at-or-past the box edge pages) keeps working while a
        // deliberate drag out of the box ejects the member: from a grid
        // box into the loose grid, from a dock stack down onto the dock
        // (drop there pins it) or into the grid.
        let m = content::GRID_CELL_H * 0.5;
        let clear_out = pos.0 < box_rect.x - m
            || pos.0 > box_rect.x + box_rect.w + m
            || pos.1 < box_rect.y - m
            || pos.1 > box_rect.y + box_rect.h + m;
        if clear_out {
            let id = self.entries[entry_idx].id.clone();
            self.box_drag = None;
            self.box_drag_page_at = None;
            self.pull_box_member_out(&id, pos);
            return;
        }
        self.box_drag = Some((entry_idx, pos));
        self.box_drag_edge_page(box_rect, pos);
        self.schedule_frame();
    }

    /// Edge-paging for an in-box reorder drag: holding the drag in the
    /// box's edge band turns the page every [`DRAG_PAGE_COOLDOWN`]
    /// (shared band + dwell with the Apps grid), cycling member pages →
    /// the single ghost page → back to the first. Called on motion *and*
    /// every frame from `draw` (via the stored drag pos) so paging keeps
    /// going while the pointer holds still at the edge.
    pub(crate) fn box_drag_edge_page(&mut self, box_rect: content::Rect, pos: (f32, f32)) {
        let dir = edge_page_dir(box_rect.x, box_rect.w, pos.0);
        if edge_page_due(&mut self.box_drag_page_at, dir) {
            self.turn_box_page(dir, true);
        }
        if dir != 0 {
            // Keep frames coming while the dwell clock runs (a stationary
            // pointer at the edge emits no motion).
            self.dirty = true;
        }
    }

    /// Drop the in-box drag: the member lands on the box page under the
    /// pointer — before the member displayed at that 3×3 position, or
    /// appended to the page's tail. A ghost-page drop creates the page,
    /// and the member *stays* there: box pages keep gaps like the grid.
    pub(crate) fn drop_box_drag(&mut self) {
        let Some((entry_idx, pos)) = self.box_drag.take() else {
            return;
        };
        self.box_drag_page_at = None;
        let Some(g) = self.open_box_group() else {
            return;
        };
        let member_id = self.entries[entry_idx].id.clone();
        let Some(target) = self.box_drag_slot_target(pos) else {
            self.schedule_frame();
            return;
        };
        let page = target / groups::PAGE_CAP;
        // The member displayed at the drop position, resolved by the
        // shared drop rule over the compacted display slots (dragged
        // member excluded — it's the ghost in hand, and its page packs
        // dense around the hole).
        let anchor = self.groups.groups().get(g).and_then(|grp| {
            let items = grp.members.pages().iter().enumerate().flat_map(|(p, pg)| {
                pg.iter()
                    .filter(|m| m.as_str() != member_id)
                    .enumerate()
                    .map(move |(w, m)| (p * groups::PAGE_CAP + w, m))
            });
            pages::drop_anchor(items, target, groups::PAGE_CAP).cloned()
        });
        info!("box drop: {member_id} -> slot {target} anchor {anchor:?}");
        match anchor {
            Some(a) => self.groups.move_member_before(g, &member_id, &a),
            None => self.groups.move_member_to_page_end(g, &member_id, page),
        }
        // The dropped member re-enters the glide at rest in its new slot
        // (it was the ghost in hand — gliding in from its old slot would
        // look like a bounce).
        self.box_slide.retain(|(e, _)| *e != entry_idx);
        self.refilter();
        self.schedule_frame();
    }

    /// Display slot under the pointer: the 3×3 position on the current
    /// box page, as `page * PAGE_CAP + within`.
    pub(crate) fn box_drag_slot_target(&self, pos: (f32, f32)) -> Option<usize> {
        let box_rect = self.current_layout().open_box?;
        let cols = content::OPEN_BOX_COLS;
        let cell = box_rect.w / cols as f32;
        let col = (((pos.0 - box_rect.x) / cell).floor() as usize).min(cols - 1);
        let row = (((pos.1 - box_rect.y) / cell).floor() as usize).min(cols - 1);
        let slot = row * cols + col;
        let local = content::open_box_slot_member(self.box_is_left(), slot);
        Some(self.box_page * groups::PAGE_CAP + local)
    }

    /// Pull a box member out into the loose grid (the box shrinks closed).
    pub(crate) fn pull_box_member_out(&mut self, member_id: &str, pos: (f32, f32)) {
        let Some(g) = self.open_box_group() else {
            return;
        };
        info!("box pull-out: {member_id} leaves the box, riding the drag");
        let Some(entry_idx) = self.entries.iter().position(|e| e.id == member_id) else {
            return;
        };
        let group = &self.groups.groups()[g];
        let gid = format!("group:{}", group.id);
        // A two-member box dissolves the moment one leaves. If it was a
        // dock stack, its lone survivor inherits the folder's dock pin so
        // it doesn't silently fall into the grid.
        let survivor = (group.members.len() == 2)
            .then(|| {
                group
                    .members
                    .iter()
                    .find(|m| m.as_str() != member_id)
                    .cloned()
            })
            .flatten();
        // The box's remaining members feed the shrinking-closed overlay.
        let remaining: Vec<usize> = group
            .members
            .iter()
            .filter(|id| id.as_str() != member_id)
            .take(groups::PAGE_CAP)
            .filter_map(|id| self.entries.iter().position(|e| &e.id == id))
            .collect();
        let from_dock_stack = self.dock_stack.is_some();
        self.groups.remove_member(member_id);
        match (from_dock_stack, survivor) {
            (true, Some(surv)) => {
                // The dock stack dissolved: hand its pin slot to the
                // survivor and drop the box instantly (its group — the
                // shrink anchor — no longer exists).
                if let Some(slot) = self.pins.pins().iter().position(|p| *p == gid) {
                    self.pins.pin_at(&surv, slot);
                }
                self.pins.unpin(&gid);
                self.dock_stack = None;
                self.closing_members = None;
                self.group_anim = 0.0;
                self.group_anim_target = 0.0;
            }
            (true, None) => {
                // The stack lives on: keep it as the shrink anchor above
                // the dock; the settle in `draw` clears it.
                self.group_anim_target = 0.0;
            }
            (false, _) => {
                // Grid box: drop the logical open state so the grid comes
                // alive, but keep the shrink animating over it.
                self.app_group = None;
                self.closing_members = Some(remaining);
                self.group_anim_target = 0.0;
            }
        }
        self.refilter();
        self.gesture.dragging = Some(DragState {
            entry_idx,
            from_dock: false,
            pos,
        });
        self.gesture.pressed = None;
        self.schedule_frame();
    }

    /// Accumulate scroll toward turning the open box's 3×3 page — the
    /// box pager's own wheel accumulator (it no longer borrows the Apps
    /// section's, so box scrolling can't contaminate grid paging).
    pub(crate) fn box_page_scroll(&mut self, value: f64) {
        if let Some(dir) = self.box_pager.wheel(value) {
            self.turn_box_page(dir, true);
        }
    }

    /// Turn the open box a page (+1 next, -1 previous): wheel paging
    /// wraps cyclically; drag paging cycles too (with its single ghost
    /// page — see [`pager::Pager::turn`]).
    pub(crate) fn turn_box_page(&mut self, dir: i64, wrap: bool) {
        let Some(n) = self.stack_members().map(|m| m.pages().len()) else {
            return;
        };
        // While reordering a member, one extra empty page is reachable
        // at the end — drop there to park the member on a fresh page.
        let pages = n.max(1) + usize::from(self.box_drag.is_some());
        let page_w = self
            .layout_at(self.ui.extent_of(Target::Open))
            .open_box
            .map_or(1.0, |b| b.w);
        if self.box_pager.turn(dir, wrap, pages, page_w) {
            self.box_page = self.box_pager.page(page_w).min(pages - 1);
            self.schedule_frame();
        }
    }
}

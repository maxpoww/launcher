//! Frame assembly: the per-frame `draw` (animation stepping, make-room
//! glides, scene-input building, render submission) and the layout
//! helpers every other module reads the geometry through.

use std::time::Instant;

use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{debug, error};

use crate::install::FAIL_FLASH;
use crate::state::Target;
use crate::{animation, apps, content, groups, hypr, pager, pages};
use crate::{App, BOUNCE_DURATION, BOUNCE_HEIGHT, DOCK_REST_AFTER_CLOSE};

/// Exponential make-room glide rate (per second) for the grid and dock
/// reflow while dragging — higher is snappier, lower is slower.
pub(crate) const MAKEROOM_RATE: f32 = 18.0;

impl App {
    /// Layout for an arbitrary card extent at the current scroll offsets.
    /// Whether a drag that can land on the Apps grid is in flight — the
    /// grid then offers one extra empty page at its end (the Launchpad
    /// gesture: drag to the edge to park an app on a fresh page).
    pub(crate) fn ghost_page_active(&self) -> bool {
        self.search.query.is_empty()
            && self.gesture.dragging.as_ref().is_some_and(|d| {
                matches!(
                    self.kinds.get(d.entry_idx),
                    Some(apps::EntryKind::App)
                        | Some(apps::EntryKind::Group)
                        | Some(apps::EntryKind::Package)
                )
            })
    }

    pub(crate) fn layout_at(&self, extent: f32) -> content::Layout {
        // The Apps section is paged by display *span* (tail gaps count
        // toward their page), not item count; a qualifying drag adds one
        // ghost page to drag onto.
        let apps_cells = if self.ghost_page_active() {
            self.apps_span + self.apps_cap.max(1)
        } else {
            self.apps_span
                .max(self.search.visible[content::SECTION_APPS].len())
        };
        let mut layout = content::layout(
            &self.config,
            self.icon_scale(),
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            extent,
            self.dock_order.len(),
            [
                apps_cells,
                self.search.visible[content::SECTION_INSTALL].len(),
                self.search.visible[content::SECTION_FILES].len(),
            ],
            std::array::from_fn(|s| self.scroll.per[s].pos),
            [
                self.stack_open() || self.closing_members.is_some(),
                false,
                self.files_dir.is_some(),
            ],
            (self.agua_card.pos, self.agua_content.pos),
        );
        // Position the open box's rest square. A grid box anchors to the
        // side of the grid it sits on (pinned preview icon lands on its
        // closed spot); a dock stack floats the same-size square above its
        // dock folder icon.
        if let (Some(base), Some((ox, oy))) = (layout.open_box, self.group_origin) {
            let vp = layout.sections[content::SECTION_APPS].viewport;
            let s = base.h;
            // The dock icon a dock-anchored stack grows out of: a folder's
            // box id, or a pinned directory's entry id. A dir stack opened
            // with the card up anchors into the grid instead (the grid-box
            // path below), like a dock folder does then.
            let dock_anchor: Option<String> = if let Some(g) = self.dock_stack {
                Some(format!("group:{}", self.groups.groups()[g].id))
            } else {
                self.dir_stack
                    .as_ref()
                    .filter(|ds| !ds.in_grid)
                    .map(|ds| ds.id.clone())
            };
            if let Some(aid) = dock_anchor {
                let dock = self
                    .dock_order
                    .iter()
                    .position(|&e| self.entries.get(e).is_some_and(|x| x.id == aid))
                    .and_then(|sl| layout.dock_slots.get(sl));
                if let Some(&dock_rect) = dock {
                    // Use the same side as the grid box (3 scaled cells) so
                    // dock stacks and grid boxes are identical in size.
                    let side = content::OPEN_BOX_COLS as f32
                        * content::GRID_CELL_H
                        * self.icon_scale();
                    layout.open_box = Some(content::dock_box_rect(
                        dock_rect,
                        side,
                        self.buffer_size.0 as f32,
                    ));
                }
            } else {
                let half_cell = s / 6.0; // half a 3×3 cell
                let mini_off = content::GRID_ICON * self.icon_scale() * 0.26;
                let left = ox < vp.x + vp.w / 2.0;
                // Place the near-corner cell center on the pinned icon's spot.
                let x = if left {
                    ox - mini_off - half_cell
                } else {
                    ox + mini_off - (s - half_cell)
                };
                let y = oy - mini_off - half_cell;
                let x = x.clamp(vp.x, (vp.x + vp.w - s).max(vp.x));
                let y = y.clamp(vp.y, (vp.y + vp.h - s).max(vp.y));
                layout.open_box = Some(content::Rect::new(x, y, s, s));
            }
        }
        layout
    }

    /// Layout for the current animation extent.
    pub(crate) fn current_layout(&self) -> content::Layout {
        self.layout_at(self.ui.extent())
    }

    /// Current upward offset of the launch bounce, expiring it when
    /// done: two hops decaying in height.
    pub(crate) fn bounce_offset(&mut self) -> Option<(usize, f32)> {
        let (index, start) = self.bounce?;
        let t = start.elapsed().as_secs_f32() / BOUNCE_DURATION.as_secs_f32();
        if t >= 1.0 {
            self.bounce = None;
            return None;
        }
        let hops = (2.0 * std::f32::consts::PI * t).sin().abs();
        Some((index, BOUNCE_HEIGHT * hops * (1.0 - 0.45 * t)))
    }

    /// Render one frame and request the next frame callback, which rides
    /// on this frame's commit. The callback redraws only if the scene is
    /// animating or damaged again — otherwise it fires clean and the
    /// daemon goes fully idle (no further frame requests).
    pub(crate) fn draw(&mut self) {
        if self.renderer.is_none() {
            return;
        }

        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.last_frame = Some(now);

        let was_animating = self.ui.is_animating();
        let animating = self.ui.tick(dt);
        // AGUA: the card's speed pours energy into three water bodies —
        // the silhouette, the dock icons, and the content — each chasing
        // the same target at its own tempo, so their swells peak at
        // slightly different moments and keep sloshing briefly after the
        // card lands, all relaxing to exactly 1.0.
        let stretch_target = 1.0
            + (self.ui.progress_velocity() * content::STRETCH_PER_VEL)
                .clamp(-content::SQUASH_MAX, content::STRETCH_MAX);
        self.agua_card.step(stretch_target, dt);
        self.agua_icons.step(stretch_target, dt);
        self.agua_content.step(stretch_target, dt);
        self.agua_breath.step(stretch_target, dt);
        // Drain delayed jelly kicks (anticipation, main, Poisson cross-coupling),
        // then step both membranes' edge springs toward rest. They always target
        // rest (1.0) and settle independently of the main animation.
        self.jelly.drain(dt);
        self.box_jelly.drain(dt);
        self.jelly.step(dt);
        self.box_jelly.step(dt);
        let stretch_active = self.agua_card.is_active()
            || self.agua_icons.is_active()
            || self.agua_content.is_active()
            || self.agua_breath.is_active()
            || self.jelly.is_active()
            || self.box_jelly.is_active();
        if !stretch_active
            && !animating
            && !self.jelly.has_pending()
            && !self.box_jelly.has_pending()
        {
            self.agua_card.snap();
            self.agua_icons.snap();
            self.agua_content.snap();
            self.agua_breath.snap();
            self.jelly.snap();
            self.box_jelly.snap();
        }
        if animating {
            // The card is moving under a possibly stationary pointer:
            // keep the hover highlight glued to what is really beneath it.
            // (Not update_hover(): its schedule_frame would recurse here.)
            self.hover = self.hover_at_pointer();
        }
        if was_animating && !animating {
            // Settled: correct the input region to the final rest point.
            // Doing this only on settle (not every frame) avoids a
            // set-region + surface commit per frame of the slide, which
            // stuttered the animation; the region set at the transition
            // start already covers the visible card for the whole move.
            self.sync_input_region();
            // The region just shrank; a cursor parked over the old card
            // gets no leave from this compositor, so clear it now or the
            // dock stays stuck visible (no dodge / no auto-hide) until the
            // mouse moves.
            self.reconcile_stale_pointer();
            // The layer just shrank off the cursor and stopped committing
            // frames. Force focus back to the window we opened over now:
            // this is the quiet moment where a forced re-bind of the
            // keyboard seat won't be clobbered by follow_mouse or an
            // in-flight surface commit.
            if let Some(addr) = self.pending_refocus.take() {
                debug!("settled ({:?}); forcing focus to {addr}", self.ui.target());
                hypr::focus_window(&addr);
            }
            // A box close settling to the dock rests a beat, then hides
            // (the timer's guard keeps it if the pointer is on the dock).
            if self.rest_hide_pending && self.ui.target() == Target::Dock {
                self.rest_hide_pending = false;
                if !self.zone_free {
                    self.schedule_hide_after(DOCK_REST_AFTER_CLOSE);
                }
            }
        }

        // Advance search-box expand animation (200 ms).
        let search_target = if self.search.open { 1.0f32 } else { 0.0 };
        let search_animating = self.search.expand != search_target;
        if search_animating {
            let delta = dt / 0.2;
            self.search.expand = if search_target > self.search.expand {
                (self.search.expand + delta).min(1.0)
            } else {
                (self.search.expand - delta).max(0.0)
            };
        }

        // Smooth-scroll each section and the open box — one Pager slide.
        let mut scroll_animating = false;
        for sec in &mut self.scroll.per {
            scroll_animating |= sec.ease(dt);
        }
        scroll_animating |= self.box_pager.ease(dt);

        // Jelly edge: agua_breath is the same spring impulse as agua_card but
        // softer (lower k, c) so it rings longer after the box opens.
        // Perfectly in sync — same stretch_target, just slower to settle.
        let breath_offset = if self.ui.target() == crate::state::Target::Open {
            (self.agua_breath.pos - 1.0) * 40.0
        } else {
            0.0
        };

        self.dirty = false;

        let wl_surface = self.layer.wl_surface();
        wl_surface.frame(&self.qh, wl_surface.clone());
        self.frame_pending = true;

        let bounce = self.bounce_offset();
        let layout = self.current_layout();

        // The search caret anchors to the query's shaped width.
        let query_px = self
            .renderer
            .as_mut()
            .map(|r| r.measure_text(&self.search.query, content::SEARCH_FONT_PX))
            .unwrap_or(0.0);
        // (layout.scroll is the cyclic-wrapped image of list_scroll; the
        // raw value is what animates, so never sync it back from layout.)
        // Grid drag: track the make-room gap under the pointer and the
        // fold target (ring); remember which cell to hide (the ghost
        // is its visual).
        let over_cell = self.update_grid_target(&layout);
        // In-box reorder drag: keep paging while the pointer holds at the
        // box edge (motion alone would stall the dwell). Pull-out stays
        // motion-driven — a deliberate gesture always carries motion.
        if let (Some((_, pos)), Some(box_rect)) = (self.box_drag, layout.open_box) {
            self.box_drag_edge_page(box_rect, pos);
        }
        // Dock fold target: an app dragged over another dock icon's center
        // (not its own) will join/create a box there.
        let over_dock = self.gesture.dragging.as_ref().and_then(|d| {
            if self.kinds.get(d.entry_idx) != Some(&apps::EntryKind::App) {
                return None;
            }
            let slot = self.dock_fold_target(&layout, d.pos)?;
            (self.dock_order.get(slot) != Some(&d.entry_idx)).then_some(slot)
        });
        // The icon in hand is the ghost: hide its resting cell — the
        // grid cell for grid-origin drags, the dock slot for
        // dock-origin ones.
        let dock_hidden = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| d.from_dock)
            .map(|d| d.entry_idx);
        let drag_hidden = self.gesture.dragging.as_ref().and_then(|drag| {
            (!drag.from_dock
                && matches!(
                    self.kinds.get(drag.entry_idx),
                    Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
                ))
            .then_some(drag.entry_idx)
        });
        // Make-room glide: each Apps cell eases toward its display
        // slot (the gap starts at the pickup slot, so nothing moves
        // until the pointer asks for room). ~90 ms exponential
        // ease-out — crisp, no overshoot.
        let apps_len = self.search.visible[content::SECTION_APPS].len();
        if self.apps_slide.len() != apps_len {
            self.apps_slide = (0..apps_len)
                .map(|i| self.apps_slots.get(i).copied().unwrap_or(i) as f32)
                .collect();
        }
        let orig_pos = drag_hidden.and_then(|e| {
            self.search.visible[content::SECTION_APPS]
                .iter()
                .position(|&v| v == e)
        });
        // A dock app or box dragged into the grid opens a brand-new slot
        // (it has no cell to vacate): cells at or past the gap slide down
        // one to make room — like a reorder, but leaving no hole behind.
        let insert_gap = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| {
                d.from_dock
                    && matches!(
                        self.kinds.get(d.entry_idx),
                        Some(apps::EntryKind::App | apps::EntryKind::Group)
                    )
            })
            .and(self.reorder_slot);
        // Shifts happen in slot space, page-locally (Launchpad), via the
        // shared reflow ([`pages::shifted_slot`] — the box uses it too).
        let cap = self.apps_cap.max(1);
        let orig_slot = orig_pos.and_then(|op| self.apps_slots.get(op).copied());
        let mut slide_animating = false;
        for i in 0..apps_len {
            let s = self.apps_slots.get(i).copied().unwrap_or(i);
            let mut target = s as f32;
            if let (Some(op), Some(o)) = (orig_pos, orig_slot) {
                if i != op {
                    let g = self.reorder_slot.unwrap_or(o);
                    target = pages::shifted_slot(s, Some(o), g, cap) as f32;
                }
            } else if let Some(g) = insert_gap {
                target = pages::shifted_slot(s, None, g, cap) as f32;
            }
            let (v, moving) =
                animation::ease_toward(self.apps_slide[i], target, dt, MAKEROOM_RATE, 0.005);
            self.apps_slide[i] = v;
            slide_animating |= moving;
        }
        // Dock make-room glide, same idea in dock-slot units: dragging
        // over the dock parts the icons around the insertion point;
        // a dock-origin drag's gap rests at its old slot meanwhile.
        let dock_insert_now = self
            .gesture
            .dragging
            .as_ref()
            .filter(|_| over_dock.is_none()) // folding onto an icon: don't part
            .and_then(|d| match self.kinds.get(d.entry_idx) {
                Some(apps::EntryKind::App | apps::EntryKind::File | apps::EntryKind::Group) => {
                    self.drag_dock_insert(&layout, d.pos)
                }
                _ => None,
            });
        let n_dock = layout.dock_slots.len();
        if self.dock_slide.len() != n_dock {
            self.dock_slide = vec![0.0; n_dock];
        }
        let dock_origin = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| d.from_dock)
            .and_then(|d| self.dock_order.iter().position(|&e| e == d.entry_idx))
            .filter(|&o| o < n_dock);
        for kk in 0..n_dock {
            let target = match (dock_insert_now, dock_origin) {
                (Some(g), Some(o)) => {
                    if kk == o {
                        0.0
                    } else {
                        let compact = if kk > o { kk - 1 } else { kk };
                        let g_c = g - usize::from(o < g);
                        (compact + usize::from(compact >= g_c)) as f32 - kk as f32
                    }
                }
                // A foreign icon hovers: part the row around the slot.
                (Some(g), None) => {
                    if kk >= g {
                        0.5
                    } else {
                        -0.5
                    }
                }
                _ => 0.0,
            };
            let (v, moving) =
                animation::ease_toward(self.dock_slide[kk], target, dt, MAKEROOM_RATE, 0.005);
            self.dock_slide[kk] = v;
            slide_animating |= moving;
        }
        if slide_animating {
            self.dirty = true;
        }
        // Magnification is dead while dragging and stays dead for a
        // beat after a drop — the landing must be perfectly still.
        // It never pops back either: an amplitude envelope fades it
        // in over ~350 ms once the sleep ends (and out on drag start).
        if let Some(until) = self.mag_sleep {
            if Instant::now() >= until {
                self.mag_sleep = None;
            } else {
                // Keep frames coming so the wake-up isn't missed.
                self.dirty = true;
            }
        }
        let mag_target = if self.gesture.dragging.is_none() && self.mag_sleep.is_none() {
            1.0f32
        } else {
            0.0
        };
        if (self.mag_amount - mag_target).abs() > 0.005 {
            let step = dt / 0.35;
            self.mag_amount = if mag_target > self.mag_amount {
                (self.mag_amount + step).min(1.0)
            } else {
                // Fading out fast keeps drag starts crisp.
                (self.mag_amount - step * 3.0).max(0.0)
            };
            self.dirty = true;
        } else {
            self.mag_amount = mag_target;
        }
        let mag_pointer = if self.mag_amount > 0.0 {
            self.pointer_pos
        } else {
            None
        };

        // AGUA splash ripple (decoration only — the hover magnification in
        // scene() is untouched). The dock is a shallow 1-D water surface:
        // one height per icon, coupled to its neighbors. Nothing feeds it
        // while a crest simply sits under the pointer; only when a crest
        // *collapses* (pointer leaves or jumps) does the falling swell push
        // the surface down, and that dent propagates outward as an
        // expanding ripple, reflecting off the dock ends and decaying.
        let ripple_layout = self.current_layout();
        let n_dock = ripple_layout.dock_slots.len();
        if self.dock_wave_h.len() != n_dock {
            self.dock_wave_h = vec![0.0; n_dock];
            self.dock_wave_v = vec![0.0; n_dock];
            self.dock_crest_prev = vec![0.0; n_dock];
        }
        let mag = self.mag_amount.clamp(0.0, 1.0);
        for (i, slot) in ripple_layout.dock_slots.iter().enumerate() {
            // Crest fraction (0..1) — the same falloff scene() uses, but
            // read here only to detect its *fall*, never to magnify.
            let crest = match self.pointer_pos {
                Some((px, py)) if mag > 0.0 => {
                    let cx = slot.x + slot.w / 2.0;
                    let d_out = (slot.y - py)
                        .max(py - ripple_layout.dock_hit_bottom)
                        .max(0.0);
                    let fy = content::falloff(d_out, content::DOCK_MAG_VRADIUS);
                    content::falloff(px - cx, content::DOCK_MAG_RADIUS) * fy * mag
                }
                _ => 0.0,
            };
            let drop = (self.dock_crest_prev[i] - crest).max(0.0);
            self.dock_wave_v[i] -= content::SPLASH_GAIN * drop;
            self.dock_crest_prev[i] = crest;
        }
        let mut remaining = dt.min(0.25);
        while remaining > 0.0 {
            let h = remaining.min(1.0 / 240.0);
            for i in 0..n_dock {
                // Reflective ends: a missing neighbor mirrors the cell.
                let left = self.dock_wave_h[i.saturating_sub(1)];
                let right = self.dock_wave_h[(i + 1).min(n_dock - 1)];
                let lap = left + right - 2.0 * self.dock_wave_h[i];
                let accel = content::RIPPLE_COUPLE * lap
                    - content::RIPPLE_RETURN * self.dock_wave_h[i]
                    - content::RIPPLE_DAMP * self.dock_wave_v[i];
                self.dock_wave_v[i] += accel * h;
            }
            for i in 0..n_dock {
                self.dock_wave_h[i] += self.dock_wave_v[i] * h;
            }
            remaining -= h;
        }
        let mut ripple_active = false;
        let dock_ripple: Vec<f32> = self
            .dock_wave_h
            .iter()
            .zip(&self.dock_wave_v)
            .map(|(&hh, &vv)| {
                ripple_active |= hh.abs() > 0.001 || vv.abs() > 0.01;
                hh.clamp(-content::RIPPLE_MAX, content::RIPPLE_MAX)
            })
            .collect();

        // Box open/close transition (duration from config, eased below).
        if self.group_anim != self.group_anim_target {
            let secs = (self.config.animation.group_expand_ms as f32 / 1000.0).max(0.001);
            let step = dt / secs;
            if self.group_anim_target > self.group_anim {
                self.group_anim = (self.group_anim + step).min(self.group_anim_target);
            } else {
                self.group_anim = (self.group_anim - step).max(self.group_anim_target);
            }
            if self.group_anim <= 0.0 && self.group_anim_target <= 0.0 {
                // Fully collapsed back into the tile: leave the box (grid or
                // dock stack) and end any drag-out shrink.
                let was_dock = self.dock_stack.is_some() || self.dir_stack.is_some();
                self.app_group = None;
                self.dock_stack = None;
                self.dir_stack = None;
                self.closing_members = None;
                self.group_origin = None;
                self.group_anim = 1.0;
                self.group_anim_target = 1.0;
                if was_dock {
                    self.sync_input_region();
                }
                self.refilter();
            } else {
                self.dirty = true;
            }
        }
        let group_expand = {
            let t = self.group_anim.clamp(0.0, 1.0);
            // Smootheststep (7th order): zero velocity, acceleration *and*
            // jerk at both ends — an extra-gentle takeoff and landing.
            t * t * t * t * (35.0 + t * (-84.0 + t * (70.0 + t * -20.0)))
        };

        let drag_frame = self
            .gesture
            .dragging
            .as_ref()
            .map(|drag| content::DragFrame {
                entry_idx: drag.entry_idx,
                // A dock-origin drag is locked inside the box: the ghost
                // can reorder or drop into the grid, but never follows the
                // pointer up/out of the card (dropping outside just snaps
                // back). Grid/Install drags are unclamped.
                pos: if drag.from_dock {
                    let right = self.buffer_size.0 as f32 - content::DRAG_MARGIN_X;
                    (
                        drag.pos.0.clamp(content::DRAG_MARGIN_X, right),
                        drag.pos
                            .1
                            .clamp(layout.card_top, layout.card_top + layout.card_h),
                    )
                } else {
                    drag.pos
                },
                drop_section: self.drag_drop_section(&layout, drag.pos, drag.entry_idx),
                over_cell,
                over_dock,
            });
        // A pending drag-to-install tile counts as busy (its "Installing…"
        // note holds from the drop until the rescan swaps in the real app)
        // unless the install failed, when it flashes "Failed" and awaits a
        // retry click.
        let installing: Vec<bool> = self
            .entries
            .iter()
            .map(|e| {
                self.pending_installs
                    .iter()
                    .any(|p| p.attr == e.id && !p.failed)
            })
            .collect();
        // Packages being realized for an ephemeral run show "Launching…".
        let launching: Vec<bool> = self
            .entries
            .iter()
            .map(|e| self.launching.contains(&e.id))
            .collect();
        let busy: Vec<bool> = self
            .entries
            .iter()
            .zip(&installing)
            .map(|(e, &inst)| inst || self.busy_ids.contains(&e.id))
            .collect();
        let failed: Vec<bool> = self
            .entries
            .iter()
            .map(|e| {
                self.failed_ids
                    .get(&e.id)
                    .is_some_and(|t| t.elapsed() < FAIL_FLASH)
                    || self
                        .pending_installs
                        .iter()
                        .any(|p| p.attr == e.id && p.failed)
            })
            .collect();
        // While a box is open (or shrinking closed after a drag-out), its
        // members (up to nine) fill the magnified box's 3×3 app grid.
        // An in-box reorder drag: hide the dragged member and open a gap at
        // the slot under the pointer (rendered by `scene`).
        let box_drag = self
            .box_drag
            .and_then(|(entry, pos)| self.box_drag_slot_target(pos).map(|gap| (entry, gap, pos)));
        let (open_box_members, open_box_slots): (Vec<usize>, Vec<usize>) = self
            .stack_members()
            .map(|members| {
                // All members with their display slots (page * PAGE_CAP
                // + within — pages may be under-full); the box overlay
                // pages them 3×3 via box_scroll.
                members
                    .pages()
                    .iter()
                    .enumerate()
                    .flat_map(|(p, page)| {
                        page.iter()
                            .enumerate()
                            .map(move |(w, id)| (p * groups::PAGE_CAP + w, id))
                    })
                    .filter_map(|(slot, id)| {
                        self.entries
                            .iter()
                            .position(|e| &e.id == id)
                            .map(|e| (e, slot))
                    })
                    .unzip()
            })
            .or_else(|| {
                // The shrinking-closed overlay: sequential slots.
                self.closing_members.clone().map(|m| {
                    let n = m.len();
                    (m, (0..n).collect())
                })
            })
            .unwrap_or_default();
        // Box make-room glide (the counterpart of `apps_slide`): each
        // member eases toward its display slot — shifted page-locally
        // around the drag gap by the same shared reflow the grid uses —
        // so reorders glide instead of snapping. Carried by entry index,
        // so a drop's refilter keeps every icon's visual position.
        let box_gap = box_drag.map(|(_, g, _)| g);
        let box_hidden = box_drag.map(|(h, _, _)| h);
        let box_origin = box_hidden.and_then(|h| {
            open_box_members
                .iter()
                .position(|&e| e == h)
                .and_then(|m| open_box_slots.get(m).copied())
        });
        let mut box_sliding = false;
        let mut open_box_disp: Vec<f32> = Vec::with_capacity(open_box_members.len());
        let mut new_box_slide: Vec<(usize, f32)> = Vec::with_capacity(open_box_members.len());
        for (m, &e) in open_box_members.iter().enumerate() {
            let s = open_box_slots.get(m).copied().unwrap_or(m);
            let target = match box_gap {
                Some(g) if Some(e) != box_hidden => {
                    pages::shifted_slot(s, box_origin, g, groups::PAGE_CAP) as f32
                }
                _ => s as f32,
            };
            let cur = self
                .box_slide
                .iter()
                .find(|(ee, _)| *ee == e)
                .map(|&(_, d)| d)
                .unwrap_or(target);
            let (next, moving) = animation::ease_toward(cur, target, dt, MAKEROOM_RATE, 0.005);
            box_sliding |= moving;
            new_box_slide.push((e, next));
            open_box_disp.push(next);
        }
        self.box_slide = new_box_slide;
        if box_sliding {
            self.dirty = true;
        }
        // Hide the open box's own tile from the grid behind — the magnified
        // box stands in for it.
        let open_box_hidden = self.app_group.and_then(|g| {
            let gid = format!("group:{}", self.groups.groups().get(g)?.id);
            self.entries.iter().position(|e| e.id == gid)
        });
        let open_box_pages = self
            .stack_members()
            .map(|members| {
                // One reachable ghost page while a member is in hand.
                let pages = members.pages().len().max(1) + usize::from(self.box_drag.is_some());
                (self.box_page.min(pages - 1), pages)
            })
            .unwrap_or((0, 1));
        // Per-slot running flags for the macOS indicator dot.
        let dock_running: Vec<bool> = self
            .dock_order
            .iter()
            .map(|e| self.running.contains_key(e))
            .collect();
        let scene = content::scene(
            &self.config,
            self.icon_scale(),
            &layout,
            &self.entries,
            &self.search.visible,
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            &content::FrameInput {
                // Suppress hover highlight and magnification while dragging.
                hover: if drag_frame.is_none() {
                    self.hover
                } else {
                    None
                },
                query_px,
                stretch: self.agua_icons.pos,
                dock_tooltip: if drag_frame.is_none() {
                    self.dock_tooltip()
                } else {
                    None
                },
                alpha: self.ui.alpha(),
                pointer: mag_pointer,
                mag_amount: self.mag_amount,
                dock_ripple: &dock_ripple,
                bounce,
                query: &self.search.query,
                selected: self.search.selected.and_then(|i| self.flat_to_pos(i)),
                search_expand: self.search.expand,
                placeholders: &self.placeholders,
                layers: &self.icon_layers,
                files_path: &self.files_path_display(),
                dock_order: &self.dock_order,
                dock_running: &dock_running,
                dock_divider: self.dock_divider,
                drag: drag_frame,
                install_hint: self.install_hint(),
                busy: &busy,
                failed: &failed,
                installing: &installing,
                launching: &launching,
                group_minis: &self.group_minis,
                apps_group: &self.apps_group_name(),
                apps_slide: &self.apps_slide,
                drag_hidden,
                dock_hidden,
                dock_slide: &self.dock_slide,
                group_expand,
                group_origin: self.group_origin,
                open_box_members: &open_box_members,
                open_box_disp: &open_box_disp,
                open_box_hidden,
                open_box_pages,
                box_scroll: self.box_pager.pos,
                box_drag,
                breath_offset,
                card_push: self.jelly.offsets(),
                box_push: self.box_jelly.offsets(),
                card_open: self.ui.open_progress(),
                // Card drop shadow: present whenever revealed — the dock when
                // docked and the main box when open — only fading out when hidden.
                card_shadow: self.ui.reveal(),
                // A box floating on the dock (dock/dir stack) gets a drop
                // shadow; a grid box inside the launcher does not.
                box_over_dock: self.dock_stack.is_some()
                    || self.dir_stack.as_ref().is_some_and(|ds| !ds.in_grid),
            },
        );
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(e) = renderer.render(
            &scene,
            self.config.theme.text_rgba(),
            self.pointer_pos,
            self.config.theme.icon_squircle,
        ) {
            error!("render failed: {e:#}");
        }
        if search_animating && self.search.expand != search_target {
            self.dirty = true;
        }
        if scroll_animating
            && ((self.ui.target() == Target::Open
                && self.scroll.per.iter().any(pager::Pager::is_settling))
                // A box page-slide keeps animating whatever the card state
                // (a dock stack lives in the Dock state, not Open).
                || (self.stack_open() && self.box_pager.is_settling()))
        {
            self.dirty = true;
        }
        // AGUA: the water keeps sloshing briefly after the card lands, and
        // a splash ripple keeps traveling the dock — keep frames coming
        // until both rest.
        if stretch_active || ripple_active {
            self.dirty = true;
        }
        // Pending jelly impulses waiting on their delay timers.
        if self.jelly.has_pending() || self.box_jelly.has_pending() {
            self.dirty = true;
        }
        // Glass click ripple / box wave: keep frames coming until they expire.
        if let Some(r) = self.renderer.as_ref() {
            if r.has_active_ripple() || r.has_active_box_wave() {
                self.dirty = true;
            }
        }
    }
}

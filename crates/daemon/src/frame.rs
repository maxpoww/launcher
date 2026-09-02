//! Frame assembly: the per-frame `draw` (animation stepping, make-room
//! glides, scene-input building, render submission) and the layout
//! helpers every other module reads the geometry through.

use std::time::{Duration, Instant};

use smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{debug, error};

use crate::install::FAIL_FLASH;
use crate::state::Target;
use crate::{animation, apps, content, groups, hypr, pager, pages};
use crate::{App, BOUNCE_DURATION, BOUNCE_HEIGHT, DOCK_REST_AFTER_CLOSE};

/// Exponential make-room glide rate (per second) for the grid and dock
/// reflow while dragging — higher is snappier, lower is slower. Kept brisk
/// so the icons part right under the dragged ghost instead of lagging it.
pub(crate) const MAKEROOM_RATE: f32 = 34.0;

/// Minimum spacing between frames on a SOFTWARE (CPU) renderer — ~10fps.
/// See the F12 throttle at the top of [`App::draw`].
const SOFTWARE_FRAME_MIN: Duration = Duration::from_millis(100);

/// How long after the last pointer/keyboard event the F12 throttle stays
/// down. Covers smooth-scroll ease tails; a minutes-long install ring
/// with nobody at the wheel throttles after this.
const INPUT_ACTIVE_WINDOW: Duration = Duration::from_secs(2);

impl App {
    /// Layout for an arbitrary card extent at the current scroll offsets.
    /// Whether a drag that can land on the Apps grid is in flight — the
    /// grid then offers one extra empty page at its end (the Launchpad
    /// gesture: drag to the edge to park an app on a fresh page).
    pub(crate) fn ghost_page_active(&self) -> bool {
        self.grid_resting()
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
            self.stack_open() || self.closing_members.is_some(),
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
                    let side =
                        content::OPEN_BOX_COLS as f32 * content::GRID_CELL_H * self.icon_scale();
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
        // Reduce-motion: launch feedback is ornament, the launch itself is
        // the signal — skip the hops.
        if crate::animation::reduce_motion() {
            self.bounce = None;
            return None;
        }
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

        // F12: on a software (CPU) adapter every frame costs real cores —
        // a minutes-long install animation ran the daemon at 450% CPU in
        // the 8-core VM and throttled its own install's download ~35x.
        // While the user is actively interacting the throttle stands down
        // (scrolls/drags stay smooth); ambient animation streams get
        // spaced to SOFTWARE_FRAME_MIN with a timer. dt keeps accumulating
        // across skipped frames, so animations advance by real time.
        if self.renderer.as_ref().is_some_and(|r| r.is_software())
            && self.last_input.elapsed() > INPUT_ACTIVE_WINDOW
        {
            if let Some(remaining) = self
                .last_frame
                .and_then(|t| SOFTWARE_FRAME_MIN.checked_sub(t.elapsed()))
            {
                if self.soft_frame_timer {
                    return;
                }
                let timer = calloop::timer::Timer::from_duration(remaining);
                let armed = self
                    .loop_handle
                    .insert_source(timer, |_, _, app: &mut App| {
                        app.soft_frame_timer = false;
                        app.draw();
                        calloop::timer::TimeoutAction::Drop
                    })
                    .is_ok();
                if armed {
                    self.soft_frame_timer = true;
                    return;
                }
                // No timer = no throttle; draw rather than stall.
                error!("soft-frame timer failed to arm; drawing unthrottled");
            }
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
            // frames. Force focus back now: this is the quiet moment where a
            // forced re-bind of the keyboard seat won't be clobbered by
            // follow_mouse or an in-flight surface commit. Rofi behavior
            // (SH): if the user travelled to another workspace while we were
            // open, they STAY there — seat the keyboard on that workspace's
            // window instead of yanking them back to the origin.
            if let Some(addr) = self.pending_refocus.take() {
                match self.close_site.take() {
                    Some((ws, last)) if Some(ws) != self.restore_workspace => {
                        debug!(
                            "settled ({:?}); staying on travelled ws {ws}",
                            self.ui.target()
                        );
                        hypr::focus_workspace(ws);
                        if let Some(last) = last {
                            hypr::focus_window(&last);
                        }
                    }
                    _ => {
                        debug!("settled ({:?}); forcing focus to {addr}", self.ui.target());
                        hypr::focus_window(&addr);
                    }
                }
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
            .map(|r| r.measure_text(&self.search.query, content::SEARCH_FONT_PX, None))
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
        // A drag with no grid cell to vacate opens a brand-new slot: cells
        // at or past the gap slide down one to make room — like a reorder,
        // but leaving no hole behind. That's a dock app/box dragged in, or a
        // package / catalog webapp dragged up from the Install section (the
        // same set `update_grid_target` treats as `inserting`).
        let insert_gap = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| {
                (d.from_dock
                    && matches!(
                        self.kinds.get(d.entry_idx),
                        Some(apps::EntryKind::App | apps::EntryKind::Group)
                    ))
                    || self.kinds.get(d.entry_idx) == Some(&apps::EntryKind::Package)
                    || self.is_catalog_webapp(d.entry_idx)
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
        // Keep frames coming while any install is in flight so its progress
        // ring animates (a failed tile is static, so it doesn't count).
        if self.pending_installs.iter().any(|p| !p.failed)
            || !self.pending_webapps.is_empty()
            || self
                .install_notify
                .iter()
                .any(|n| crate::install::dock_notify_shine(n.shine_at) >= 0.0)
        {
            self.dirty = true;
        }
        // Webapp install ring lifecycle: begin the completion fill once the
        // fake build has elapsed, then retire the ring (revealing the app,
        // already placed) once that fill has played.
        for w in &mut self.pending_webapps {
            if w.completed_at.is_none() && w.started.elapsed() >= crate::install::WEBAPP_BUILD {
                w.completed_at = Some(Instant::now());
            }
        }
        let webapp_done: Vec<String> = self
            .pending_webapps
            .iter()
            .filter(|w| {
                w.completed_at
                    .is_some_and(|c| c.elapsed() >= crate::install::INSTALL_HOLD)
            })
            .map(|w| w.id.clone())
            .collect();
        if !webapp_done.is_empty() {
            self.pending_webapps
                .retain(|w| !webapp_done.contains(&w.id));
            for id in webapp_done {
                if self.ui.target() != Target::Open {
                    // Launcher closed: surface it on the dock with a one-shot
                    // shine (mirrors the package path in resolve_pending_installs).
                    self.install_notify.push(crate::install::InstallNotify {
                        id,
                        shine_at: Instant::now(),
                        seen_running: false,
                    });
                    if self.ui.apply(waverunner_proto::Command::Show) {
                        self.arm_notify_dock_hide(); // popped up: auto-hide in 3s
                    }
                } else {
                    self.just_installed = Some(id); // bounce it in on the grid
                }
            }
            self.recompute_dock_order();
            self.refilter();
        }
        // A finished install whose ring has now eased to full: fire the
        // deferred rescan that swaps the tile for the real app (held back so
        // the completion fill always plays — see `install_ring_progress`).
        let mut fill_done = false;
        for p in &mut self.pending_installs {
            if !p.rescan_fired
                && p.completed_at
                    .is_some_and(|c| c.elapsed() >= crate::install::INSTALL_HOLD)
            {
                p.rescan_fired = true;
                fill_done = true;
            }
        }
        if fill_done {
            self.indexer.request_rescan_fresh();
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
            // Reduce-motion: the box opens/closes in one frame (a full-range
            // step keeps the collapse-finish bookkeeping below intact).
            let step = if crate::animation::reduce_motion() {
                1.0
            } else {
                dt / secs
            };
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
                on_dock: self.drag_dock_insert(&layout, drag.pos).is_some(),
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
        // Recycle-bin reaction: the bin reddens and its lid opens while an app,
        // a box, or a file/dir is being dragged anywhere — inviting a drop
        // (apps uninstall; files go to the system trash). A package (install
        // gesture) or the bin itself don't arm it.
        let trash_react_target = self.gesture.dragging.as_ref().is_some_and(|d| {
            matches!(
                self.kinds.get(d.entry_idx),
                Some(apps::EntryKind::App | apps::EntryKind::Group | apps::EntryKind::File)
            ) && self
                .entries
                .get(d.entry_idx)
                .is_some_and(|e| !groups::is_trash(&e.id))
        });
        let (v, moving) = animation::ease_toward(
            self.trash_react,
            if trash_react_target { 1.0 } else { 0.0 },
            dt,
            14.0,
            0.004,
        );
        self.trash_react = v;
        if moving {
            self.dirty = true;
        }
        // Recycle-bin hover-grow: the bin swells while a dragged icon sits over
        // it. Computed straight from the fold target (not `over_dock`, which is
        // App-only) so a dragged file/dir hovering the bin grows it too.
        let over_trash = trash_react_target
            && self.gesture.dragging.as_ref().is_some_and(|d| {
                self.dock_fold_target(&layout, d.pos)
                    .and_then(|slot| self.dock_order.get(slot).copied())
                    .and_then(|idx| self.entries.get(idx))
                    .is_some_and(|e| groups::is_trash(&e.id))
            });
        let (hv, hv_moving) = animation::ease_toward(
            self.trash_hover,
            if over_trash { 1.0 } else { 0.0 },
            dt,
            16.0,
            0.004,
        );
        self.trash_hover = hv;
        if hv_moving {
            self.dirty = true;
        }
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
                    || self.pending_webapps.iter().any(|w| w.id == e.id)
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
        // Per-entry install visuals: the ring fraction and completion time of
        // whichever pending install/webapp matches (both share the ring +
        // shine flow).
        let install_state = |id: &str| -> (f32, Option<Instant>) {
            if let Some(p) = self
                .pending_installs
                .iter()
                .find(|p| p.attr == id && !p.failed)
            {
                (p.ring_fraction(), p.completed_at)
            } else if let Some(w) = self.pending_webapps.iter().find(|w| w.id == id) {
                (w.ring_fraction(), w.completed_at)
            } else {
                (-1.0, None)
            }
        };
        // Progress ring per entry — but suppressed once the shine sweep has
        // begun (the ring is gone by then), -1 elsewhere.
        let progress: Vec<f32> = self
            .entries
            .iter()
            .map(|e| {
                let (ring, completed) = install_state(&e.id);
                if crate::install::install_shine(completed) >= 0.0 {
                    -1.0
                } else {
                    ring
                }
            })
            .collect();
        // Shine-sweep fraction per entry: an in-flight install's post-ring
        // shine, or a just-installed dock notify's one-shot shine.
        let shine: Vec<f32> = self
            .entries
            .iter()
            .map(|e| {
                let s = crate::install::install_shine(install_state(&e.id).1);
                if s >= 0.0 {
                    s
                } else {
                    self.install_notify
                        .iter()
                        .find(|n| n.id == e.id)
                        .map(|n| crate::install::dock_notify_shine(n.shine_at))
                        .unwrap_or(-1.0)
                }
            })
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
                dock_order: &self.dock_order,
                dock_running: &dock_running,
                dock_divider: self.dock_divider,
                drag: drag_frame,
                trash_react: self.trash_react,
                trash_hover: self.trash_hover,
                install_hint: self.install_hint(),
                busy: &busy,
                failed: &failed,
                installing: &installing,
                launching: &launching,
                progress: &progress,
                shine: &shine,
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
        let thumb_base = self.thumb_layer_base();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(e) = renderer.render(
            &scene,
            self.config.theme.text_rgba(),
            self.pointer_pos,
            self.config.theme.icon_squircle,
            thumb_base,
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

    /// Handle a `configure` for the OPTIONS topbar: learn its (logical) size,
    /// build or resize its own renderer, and draw the strip once. The bar is
    /// static, so — unlike the dock — it needs no frame-callback loop yet.
    pub(crate) fn configure_options(&mut self, configure: LayerSurfaceConfigure) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return; // wait for the compositor to report a real size
        }
        self.options_size = (width, height);
        let scale = self.config.options.render_scale.max(1);
        let (pw, ph) = (width * scale, height * scale);
        if let Some(renderer) = self.options_renderer.as_mut() {
            renderer.resize(pw, ph);
        } else {
            let built = {
                let Some(layer) = self.options_layer.as_ref() else {
                    return;
                };
                crate::renderer::Renderer::new(&self.conn, layer.wl_surface(), pw, ph, scale)
            };
            match built {
                Ok(renderer) => self.options_renderer = Some(renderer),
                Err(e) => {
                    error!("options renderer init failed: {e:#}");
                    return;
                }
            }
        }
        // Now the size + renderer exist: measure the pill text (incl. the notif
        // rows, in case notifications hydrated before this first configure), set
        // the input region, and draw.
        self.measure_options_text();
        self.measure_notif();
        // Any icons resolved before this renderer existed haven't been uploaded
        // to it yet (their resolve found no renderer); push them now.
        self.upload_options_icons();
        self.sync_options_input();
        self.draw_options();
    }

    /// Draw the OPTIONS topbar. Normally a near-transparent (90%) strip; when
    /// a maximized window sits flush under it (smart gaps), the bar is painted
    /// that window's sampled top colour, opaque, so the two read as one
    /// surface (see [`crate::screencopy`]).
    pub(crate) fn draw_options(&mut self) {
        let (w, h) = self.options_size;
        if w == 0 || h == 0 {
            return;
        }
        // Concealed in fullscreen: render an empty (transparent) frame so the
        // bar disappears until a deliberate top-edge hold reveals it.
        if self.options_hidden {
            if let Some(renderer) = self.options_renderer.as_mut() {
                let scene = content::Scene {
                    alpha: 1.0,
                    ..Default::default()
                };
                let _ = renderer.render(&scene, [0.0; 4], None, 0.0, 0);
            }
            return;
        }
        let w = w as f32;
        let bar_h = self.config.options.height as f32;
        // Matched: opaque window colour, extended down over the window's top
        // border (the overhang) to hide the seam. Otherwise the faint
        // transparent strip, drawn only to the bar height.
        let (color, bottom, matched) = match self.options_bar_matched {
            Some(c) => (c, bar_h + crate::OPTIONS_OVERHANG as f32, true),
            // Reduce-transparency: the see-through strip becomes the same
            // opaque slab the open boxes use (sampled backdrop + wash), so
            // the bar's ink always sits on solid ground. Hard-edged like a
            // matched bar — an opaque fill wants a crisp bottom cut.
            None if self.config.accessibility.reduce_transparency => {
                (self.options_box_surface().0, bar_h, true)
            }
            None => ([0.0, 0.0, 0.0, 0.10], bar_h, false),
        };
        // Bleed the top/left/right edges a couple px past the surface so the
        // SDF anti-aliasing seam falls off-screen instead of showing a 1px
        // wallpaper line where an opaque matched bar meets the screen edges.
        // When matched, draw the fill hard-edged (`glass = -1.0`) so its bottom
        // edge — which meets the window's identical colour — is a crisp cut
        // rather than a gamma-lifted AA seam. See `rounded_rect.wgsl`.
        let mut scene = content::Scene {
            alpha: 1.0,
            rects: vec![content::RectInst {
                rect: content::Rect::new(-2.0, -2.0, w + 4.0, bottom + 2.0),
                radius: 0.0,
                color,
                glass: if matched { -1.0 } else { 0.0 },
                border: 0.0,
            }],
            ..Default::default()
        };
        // The context-aware pill modules ride on top of the base fill.
        self.push_options_pills(&mut scene);
        // The media transport box grows into the reserved dropdown area.
        if self.media_box_open {
            self.push_media_box(&mut scene);
        }
        // Adaptive: black text on a bright matched bar, white on a dark one.
        let text_rgba = self.options_text_color();
        let squircle = self.config.theme.icon_squircle;
        let Some(renderer) = self.options_renderer.as_mut() else {
            return;
        };
        if let Err(e) = renderer.render(&scene, text_rgba, None, squircle, 0) {
            error!("options render failed: {e:#}");
        }
    }
}

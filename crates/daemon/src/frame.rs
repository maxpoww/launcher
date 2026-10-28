//! Frame assembly: the per-frame `draw` (animation stepping, make-room
//! glides, scene-input building, render submission) and the layout
//! helpers every other module reads the geometry through.

use std::time::{Duration, Instant};

use smithay_client_toolkit::shell::wlr_layer::{LayerSurface, LayerSurfaceConfigure};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::QueueHandle;
use tracing::error;

use crate::install::FAIL_FLASH;
use crate::state::Target;
use crate::{animation, apps, content, deep_search, groups, pager, pages};
use crate::{App, BOUNCE_DURATION, BOUNCE_HEIGHT, DOCK_HOP_DURATION, DOCK_HOP_HEIGHT, DOCK_REST_AFTER_CLOSE};

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
/// How fast the overflowing pins' window slides (1/s): snappy.
const PIN_SLIDE_RATE: f32 = 20.0;
/// How fast the open card eases to a new page's height (1/s).
const TRIM_RATE: f32 = 22.0;
/// Space between the pills of the Controls row (px at bar scale 1).
const CONTROLS_GAP: f32 = 10.0;
/// The air between two of the deep search's tabs.
const TAB_GAP: f32 = 16.0;
/// How much higher the deep search's field and tabs stand than the band's
/// middle and the field's own place.
const DEEP_LIFT: f32 = 14.0;
/// How much lower than the even air the field stands, and then the field,
/// the tabs and their dots together.
const DEEP_FIELD_DOWN: f32 = 4.0;
const DEEP_DOWN: f32 = 4.0;
/// How soon to look again when the GPU is still on the previous frame (GL
/// pacing, see `Renderer::gpu_ready`).
const GPU_BUSY_RETRY: Duration = Duration::from_millis(4);
/// A frame that presented nothing gets no frame callback; an animation still
/// in flight draws again after this instead.
const UNCHANGED_RETRY: Duration = Duration::from_millis(8);

/// The time between two frames of the control panel while only its pills'
/// orbit moves (`App::orbit_soon`): about 30 a second.
const ORBIT_FRAME: Duration = Duration::from_millis(33);

/// How long a surface waits for its frame callback before it draws without
/// one (see `draw_options`). Far above any healthy frame time.
const FRAME_OVERDUE: Duration = Duration::from_millis(100);

/// What a card grown for a lone app's recents keeps clear under the bar.
const LONE_GROW_AIR: f32 = 12.0;

impl App {
    /// Layout for an arbitrary card extent at the current scroll offsets.
    /// Whether a drag that can land on the Apps grid is in flight — the
    /// grid then offers one extra empty page at its end (the Launchpad
    /// gesture: drag to the edge to park an app on a fresh page).
    ///
    /// Retired 2026-09-30 (Max: pages are created only automatically, when
    /// one overflows — see `PagedList::pack`), so never; kept as the one
    /// switch the drag code reads.
    pub(crate) fn ghost_page_active(&self) -> bool {
        const GHOST_PAGE: bool = false;
        GHOST_PAGE
            && self.grid_resting()
            && self.gesture.dragging.as_ref().is_some_and(|d| {
                matches!(
                    self.kinds.get(d.entry_idx),
                    Some(apps::EntryKind::App)
                        | Some(apps::EntryKind::Group)
                        | Some(apps::EntryKind::Package)
                )
            })
    }

    /// How much shorter the open card should be: the Apps rows the page in
    /// view leaves empty (Max, 2026-09-30: "card shrinks to fit"). Full
    /// height on the control panel; a box open in the grid gets its row's room; held
    /// while a drag is in hand so the grid doesn't move under it. And how much
    /// of a second line Files want (0 or 1): a search that left no app gives
    /// them the Apps' row when they fill more than one line
    /// (`content::files_take_apps_row`).
    pub(crate) fn apps_trim_target(&self) -> (f32, f32) {
        // (Beside a side dock the sections stand side by side: nothing to trim.)
        if self.control_panel || self.dock_side() != crate::launcher_prefs::Side::Bottom {
            return (0.0, 0.0);
        }
        let layout = self.layout_at_rest(self.ui.extent_of(Target::Open));
        let sec = &layout.sections[content::SECTION_APPS];
        let cell = content::GRID_CELL_H * self.icon_scale();
        // (The deep search's folded shelves gave the first their room.)
        let room = layout.apps_full_h.min(layout.apps_avail - self.controls_band_target()) + layout.shelf_room;
        let files = &layout.sections[content::SECTION_FILES];
        let second = if layout.apps_none && files.cells > files.cols { 1.0 } else { 0.0 };
        if self.gesture.dragging.is_some() || self.box_drag.is_some() {
            // Held, so the grid doesn't move under the drag — but a package
            // lifted out of Install over a search that left no app brings the
            // grid back, and it gets a row to be let go on.
            let held = self.ui.open_trim();
            return (if layout.apps_none { held } else { held.min(room - cell) }, second);
        }
        if layout.apps_none {
            // A search that left no app: its "No results" line, and the
            // row Files take for a second line when they fill one.
            return ((room - content::NO_APPS_H - second * cell).max(0.0), second);
        }
        // THE DEEP SEARCH's first shelf: the rows its page needs (a full
        // page all of them; nothing found, one for its words).
        if self.search.deep.is_some() && self.search.focus_file.is_none() {
            let (cols, rows) = (sec.cols.max(1), sec.rows.max(1));
            let per_page = cols * rows;
            let n = self.search.visible[content::SECTION_APPS].len();
            let page = self.scroll.per[content::SECTION_APPS].page(sec.page_len().max(1.0)) % sec.n_pages.max(1);
            let on_page = n.saturating_sub(page * per_page).min(per_page);
            let needed = if sec.n_pages > 1 { rows } else { on_page.div_ceil(cols).clamp(1, rows) };
            return ((room - needed as f32 * cell).max(0.0), 0.0);
        }
        // With more than one page every page keeps the full page's rows
        // (Max: "the lines on both pages have to be the same"); pages fill
        // before the next one exists, so only a lone page can be short.
        if sec.n_pages > 1 {
            return (0.0, 0.0);
        }
        let (cols, rows) = (sec.cols.max(1), sec.rows.max(1));
        let per_page = cols * rows;
        // (Pages wrap cyclically, so the pager's target can run past them.)
        let page = self.scroll.per[content::SECTION_APPS].page(sec.page_len().max(1.0)) % sec.n_pages.max(1);
        // The last filled slot on that page sets the rows it needs.
        // (The lone app lays its rows out by slots too: `app_actions.rs`.)
        let slots: &[usize] = if self.grid_resting() || self.lone_anchor().is_some() { &self.apps_slots } else { &[] };
        let last = if slots.is_empty() {
            // Search results: dense, one per cell.
            let n = self.search.visible[content::SECTION_APPS].len();
            n.checked_sub(page * per_page).map(|k| k.min(per_page).saturating_sub(1))
        } else {
            slots.iter().filter(|&&d| d / per_page == page).map(|&d| d % per_page).max()
        };
        let needed = last.map_or(1, |l| l / cols + 1);
        let keep = needed.min(rows);
        // A box open in the grid: the room its row takes under its tile.
        let box_room = if self.box_in_grid() {
            cell
        } else {
            // The lone app's recents' title, under its row.
            self.lone_room()
        };
        // Aimed with the room the Controls band is heading for, so the Apps
        // viewport lands on exactly `keep` rows.
        let trim = room - keep as f32 * cell - box_room;
        if trim >= 0.0 || self.app_group.is_some() || self.lone_room() <= 0.0 {
            return (trim.max(0.0), 0.0);
        }
        // A lone app's recents want more than the rows the card holds (a
        // short screen): the card GROWS by what is missing — the trim the
        // other way —, as far as the screen over it lets it.
        // (The card's top untrimmed: the trim in use moved it by as much.)
        let over = layout.card_top - self.ui.open_trim() - self.dock_top_reserved() - LONE_GROW_AIR;
        (trim.max(-over.max(0.0)), 0.0)
    }

    /// A box open in the grid (an app group, or a dock folder's stack shown
    /// there): its row takes room under its tile.
    fn box_in_grid(&self) -> bool {
        self.app_group.is_some() || self.dir_stack.as_ref().is_some_and(|ds| ds.in_grid)
    }

    /// The Controls row's pill height: the control panel's near layer.
    pub(crate) fn controls_pill_h(&self) -> f32 {
        self.options_pill_h() * self.controls_layer()
    }

    /// The size the System row's pills are drawn at: the control panel's near
    /// layer — in the deep search the band holds the search field itself
    /// (moved up, its tabs under it), at its own size.
    pub(crate) fn controls_layer(&self) -> f32 {
        crate::panel::LAYER_SCALE[if self.search.deep.is_some() { 1 } else { 0 }]
    }

    /// The room the Controls band is heading for: a row's worth while a
    /// search matches settings, none otherwise.
    pub(crate) fn controls_band_target(&self) -> f32 {
        if self.search.controls.is_empty() && !self.deep_band() {
            0.0
        } else {
            content::controls_band_h(self.icon_scale(), self.controls_pill_h()) * self.control_rows() as f32
        }
    }

    /// The band's rows: the one System row, while anything matches.
    pub(crate) fn control_rows(&self) -> usize {
        usize::from(!self.search.controls.is_empty() || self.deep_band())
    }

    /// The deep search's band: the room the search field moves up into, its
    /// tabs taking the field's place under it (`deep_search.rs`).
    pub(crate) fn deep_band(&self) -> bool {
        self.search.deep.is_some() && !self.control_panel && self.dock_side() == crate::launcher_prefs::Side::Bottom
    }

    pub(crate) fn layout_at(&self, extent: f32) -> content::Layout {
        self.layout_stretched(extent, (self.agua_card.pos, self.agua_content.pos))
    }

    /// The layout at `extent` with the card at rest: no AGUA squash or
    /// stretch. For anything that must not wobble with the card's spring
    /// (the control panel lays its field out against this).
    pub(crate) fn layout_at_rest(&self, extent: f32) -> content::Layout {
        self.layout_stretched(extent, (1.0, 1.0))
    }

    fn layout_stretched(&self, extent: f32, stretch: (f32, f32)) -> content::Layout {
        // The Apps section is paged by display *span* (tail gaps count
        // toward their page), not item count; a qualifying drag adds one
        // ghost page to drag onto.
        let apps_cells = if self.ghost_page_active() {
            self.apps_span + self.apps_cap.max(1)
        } else {
            self.apps_span
                .max(self.search.visible[content::SECTION_APPS].len())
        };
        // Aspects of the minimized tiles, dock-tail order (== `minimized`
        // order): each shapes its variable-width tile. `dock_min_count` picks
        // up how many actually draw (0 while the launcher hides them).
        let min_aspects: Vec<f32> = self.minimized.iter().map(|w| w.aspect).collect();
        let mut layout = content::layout_on(
            self.dock_side(),
            &self.config,
            self.icon_scale(),
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            extent,
            self.dock_order.len(),
            // How many of those are minimized-window tiles (the tail of the
            // order): the zone the dock widens to fit rather than clamp.
            self.dock_min_count,
            self.dock_user_pins,
            self.dock_pin_off,
            &min_aspects,
            [
                apps_cells,
                self.search.visible[content::SECTION_INSTALL].len(),
                self.search.visible[content::SECTION_FILES].len(),
            ],
            std::array::from_fn(|s| self.scroll.per[s].pos),
            self.stack_open() || self.closing_members.is_some(),
            stretch,
            (!self.search.controls.is_empty() || self.deep_band()).then(|| self.controls_pill_h()),
            self.controls_band,
            self.ui.open_trim(),
            self.control_rows(),
            self.dock_top_reserved(),
        );
        // Search results split their partial rows about the titles.
        let centre_rows = !self.grid_resting();
        for sec in &mut layout.sections {
            sec.centre_rows = centre_rows;
        }
        // A search that left no app: Apps a short line, Files two
        // (`content::files_take_apps_row`). Not under a box open in the
        // grid: that keeps its row's room.
        let bottom = !self.control_panel && layout.side == crate::launcher_prefs::Side::Bottom;
        let deep = self.search.deep.as_ref().filter(|_| bottom && centre_rows);
        layout.apps_none = bottom && centre_rows && apps_cells == 0 && !self.box_in_grid() && deep.is_none();
        if bottom && self.files_second > 0.0 {
            let extra = self.files_second * content::GRID_CELL_H * self.icon_scale();
            content::files_take_apps_row(&mut layout, extra, self.scroll.per[content::SECTION_FILES].pos);
        }
        // THE DEEP SEARCH: its empty shelves folded, their room the first's.
        if let Some(d) = deep {
            let hidden = [false, d.shelves[1].hidden, d.shelves[2].hidden];
            content::fold_shelves(&mut layout, hidden, content::GRID_CELL_H * self.icon_scale(), self.scroll.per[content::SECTION_APPS].pos);
        }
        // THE LONE APP (`app_actions.rs`): its row laid out round it, and
        // room under that row for its recents' title.
        if centre_rows {
            layout.sections[content::SECTION_APPS].anchor = self.lone_anchor();
            let room = self.lone_room();
            if room > 0.0 {
                layout.apps_part = Some(content::AppsPart { row: 0, up: 0.0, down: room });
            }
        }
        // The System row: the matching settings and modules as near-layer
        // pills, split about the title's centre (see `content::controls_rects`).
        // As many as the row's width holds, in their order: one too wide for
        // what is left is passed over and the next ones still have their
        // chance; one passed over gets no rect.
        if let Some(&(_, row)) = layout.control_bands.first() {
            let s = self.options_scale() * self.controls_layer();
            let gap = CONTROLS_GAP * self.options_scale();
            let widths: Vec<f32> = self
                .search
                .controls
                .iter()
                .map(|&(_, w)| (w + 2.0 * crate::options::PILL_PAD_X * s).max(row.h))
                .collect();
            let fits = content::row_fits(row, &widths, gap);
            let kept: Vec<f32> = fits.iter().map(|&i| widths[i]).collect();
            let mut rects = vec![content::Rect::new(0.0, 0.0, 0.0, 0.0); widths.len()];
            for (r, &i) in content::controls_rects(row, &kept, gap).into_iter().zip(&fits) {
                rects[i] = r;
            }
            layout.controls = rects;
        }
        // THE DEEP SEARCH'S TABS (`deep_search.rs`): the field moves up into
        // the band, the tabs take its place under it — the page shown, words
        // a little smaller on a box too narrow for its nine.
        if let (Some(d), Some(&(_, band)), true) = (self.search.deep.as_ref(), layout.control_bands.first(), self.deep_band()) {
            let was = layout.search_box;
            // (Both a little higher than the band's middle and the field's
            // place — Max: "move the tabs and the search a little higher".)
            let lift = DEEP_LIFT * self.options_scale();
            // (Where the tabs stood: their page dots stay at its foot.)
            let row = content::Rect::new(band.x, was.y - lift, band.w, was.h);
            let dots_y = row.y + row.h - 4.5;
            let pages = d.tags.len().div_ceil(deep_search::TABS_PAGE).max(1);
            let page = d.page.min(pages - 1);
            let gap = TAB_GAP * self.options_scale();
            // Every page laid out at rest, each centred, each its own size;
            // the page shown takes the pointer (`controls`), the slide draws
            // them where they are (`content`, `tabs_slide`).
            // (One size on every page — the one the widest page needs —, so
            // the words do not change size as the pages slide.)
            let page_of = |p: usize| p * deep_search::TABS_PAGE..((p + 1) * deep_search::TABS_PAGE).min(d.tags.len());
            let width_of = |p: usize| {
                let on = page_of(p);
                let n = on.len();
                on.map(|i| d.tab_w.get(i).copied().unwrap_or(40.0)).sum::<f32>() + gap * n.saturating_sub(1) as f32
            };
            let widest = (0..pages).map(width_of).fold(0.0f32, f32::max);
            let scale = ((row.w - 2.0 * gap) / widest.max(1.0)).min(1.0);
            let mut at = vec![content::Rect::new(0.0, 0.0, 0.0, 0.0); d.tags.len()];
            let words: Vec<f32> = (0..d.tags.len()).map(|i| d.tab_w.get(i).copied().unwrap_or(40.0) * scale).collect();
            let scales = vec![scale; pages];
            for p in 0..pages {
                let mut x = row.x + (row.w - width_of(p) * scale) / 2.0;
                for i in page_of(p) {
                    let w = d.tab_w.get(i).copied().unwrap_or(40.0);
                    at[i] = content::Rect::new(x - gap * scale / 2.0, 0.0, (w + gap) * scale, 0.0);
                    x += (w + gap) * scale;
                }
            }
            // EQUAL AIR from the last shelf's page dots to the field, the field
            // to the tabs' words, the words to their own dots (Max: "center
            // the search pill between the tabs and the dots", then "center
            // the tabs between the tabs' dots and the search box"): with `a`
            // where the shelf's dots end, `ph` the field's height, `b` the
            // words' (underline with them) and `e` where the tabs' dots
            // begin, the words begin at (a + ph + 2e - 2b) / 3 and the field
            // is centred between `a` and them.
            let first = layout.sections[content::SECTION_APPS].viewport;
            let shelf_bottom =
                layout.sections.iter().filter(|s| s.viewport.h > 0.0).map(|s| s.viewport.y + s.viewport.h).fold(first.y + first.h, f32::max);
            let a = content::deep_dots_bottom(shelf_bottom);
            let ph = self.options_pill_h();
            let b = content::deep_tab_h(scale);
            let e = dots_y - 2.5;
            let text_y = (a + ph + 2.0 * e - 2.0 * b) / 3.0;
            let field_mid = (a + text_y) / 2.0;
            // Then the field a little down, and everything a little down
            // (Max: "move search slightly down and then both search and the
            // tabs slightly down"), the tabs' dots with them.
            let both = DEEP_DOWN * self.options_scale();
            let (field_mid, text_y, dots_y) = (field_mid + DEEP_FIELD_DOWN * self.options_scale() + both, text_y + both, dots_y + both);
            layout.search_box.y = field_mid - was.h / 2.0;
            layout.search_btn.y = field_mid - was.h / 2.0;
            // (The words take the pointer a little round them.)
            for r in &mut at {
                r.y = text_y - 4.0;
                r.h = b + 8.0;
            }
            layout.tabs_row = Some(content::Rect::new(row.x, text_y - 4.0, row.w, dots_y + 4.0 - (text_y - 4.0)));
            layout.tabs_text_y = text_y;
            layout.tabs_dots_y = dots_y;
            let shown = page * deep_search::TABS_PAGE..((page + 1) * deep_search::TABS_PAGE).min(d.tags.len());
            layout.controls = (0..d.tags.len()).map(|i| if shown.contains(&i) { at[i] } else { content::Rect::new(0.0, 0.0, 0.0, 0.0) }).collect();
            layout.tabs_at = at;
            layout.tabs_words = words;
            layout.tabs_scales = scales;
            layout.tabs_page = (page, pages);
            layout.tabs_slide = d.slide;
        }
        // Place the open box. A dock stack floats over its dock icon
        // (`content::row_box`); a grid box (and a dock folder or pinned
        // directory opened with the card up) opens in the Apps grid.
        if let (Some(_), Some((_, oy))) = (layout.open_box, self.box_origin(&layout)) {
            let scale = self.icon_scale();
            let cell = content::GRID_CELL_H * scale;
            let n = self.box_total();
            // The dock icon a dock-anchored stack grows out of: a folder's
            // box id, or a pinned directory's entry id.
            let dock_anchor: Option<String> = if let Some(g) = self.dock_stack {
                Some(format!("group:{}", self.groups.groups()[g].id))
            } else {
                self.dir_stack
                    .as_ref()
                    .filter(|ds| !ds.in_grid)
                    .map(|ds| ds.id.clone())
            };
            // (Read before the closure below takes the layout to write to it.)
            let (sideways, dock_band, left) = (layout.sideways(), layout.dock_band, layout.side == crate::launcher_prefs::Side::Left);
            let placed = if let Some(aid) = dock_anchor {
                self.dock_order
                    .iter()
                    .position(|&e| self.entries.get(e).is_some_and(|x| x.id == aid))
                    .and_then(|sl| layout.dock_slots.get(sl))
                    .map(|&dock_rect| {
                        // A second dock over the dock, a gap over its
                        // plate: its members a size up, their names under.
                        // (Ctrl +/− zooms them: `zoom.rs`.)
                        let stack = scale * self.stack_zoom();
                        layout.open_box_scale = stack;
                        layout.open_box_text = self.stack_text_zoom();
                        let slot = content::STACK_CELL_W * stack;
                        let band = content::stack_cell_h(stack, layout.open_box_text);
                        let dock_h = self.config.window.input_bar_height as f32 * scale;
                        let m = content::DRAG_MARGIN_X + content::DOCK_PAD_X * scale;
                        let area = content::Rect::new(
                            m,
                            0.0,
                            (self.buffer_size.0 as f32 - 2.0 * m).max(slot),
                            layout.card_top,
                        );
                        // Beside a side dock: a column by the dock's band.
                        if sideways {
                            let m = content::DOCK_PAD_X * scale + dock_h;
                            let area = content::Rect::new(0.0, m, self.buffer_size.0 as f32, (self.buffer_size.1 as f32 - 2.0 * m).max(band));
                            return content::col_box(dock_band, dock_rect, n, (slot, band), area, content::BOX_GAP, left);
                        }
                        let plate = content::Rect::new(dock_rect.x, layout.card_top, dock_rect.w, dock_h);
                        content::row_box(plate, n, (slot, band), area, content::BOX_GAP)
                    })
            } else {
                // A box in the grid opens under its tile's row, the rows
                // below making room for it (`content::AppsPart`). Where
                // the grid can't hold it the tile's row gives the rest,
                // moving up — never past the grid's top.
                let vp = layout.sections[content::SECTION_APPS].viewport;
                // It takes whole rows' room, so a full grid stays on its
                // rows: the last ones leave whole.
                // The grid's own cells, on its columns.
                let cw = vp.w / layout.sections[content::SECTION_APPS].cols.max(1) as f32;
                let ((w, h), grid, cap) = content::row_cells(n, (cw, cell), vp.w);
                let row = if oy >= vp.y { ((oy - vp.y) / cell).floor() as isize } else { -1 };
                let split = vp.y + (row + 1) as f32 * cell;
                let up = (split + h - (vp.y + vp.h)).clamp(0.0, (row.max(0) as f32 * cell).max(0.0));
                let t = self.group_expand();
                layout.apps_part = Some(content::AppsPart { row, up: up * t, down: (h - up) * t });
                // Its members stand centred in the line, whatever column
                // the tile is in — on whole columns, under the grid's icons.
                let x = vp.x + ((cap - grid.0) / 2) as f32 * cw;
                Some((content::Rect::new(x, split - up, w, h), grid, cap))
            };
            if let Some((rect, grid, cap)) = placed {
                layout.open_box = Some(rect);
                layout.open_box_grid = grid;
                layout.open_box_cap = cap;
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

    /// The whole dock's bounce (`dock_bounce`): one hop and a smaller
    /// rebound, every icon on it together, expiring when done.
    pub(crate) fn dock_hop_offset(&mut self) -> f32 {
        if crate::animation::reduce_motion() {
            self.dock_hop = None;
            return 0.0;
        }
        let Some(start) = self.dock_hop else {
            return 0.0;
        };
        let t = start.elapsed().as_secs_f32() / DOCK_HOP_DURATION.as_secs_f32();
        if t >= 1.0 {
            self.dock_hop = None;
            return 0.0;
        }
        let hops = (2.0 * std::f32::consts::PI * t).sin().abs();
        DOCK_HOP_HEIGHT * hops * (1.0 - 0.8 * t)
    }

    /// Render one frame and request the next frame callback, which rides
    /// on this frame's commit. The callback redraws only if the scene is
    /// animating or damaged again — otherwise it fires clean and the
    /// daemon goes fully idle (no further frame requests).
    pub(crate) fn draw(&mut self) {
        if self.renderer.is_none() {
            return;
        }
        // Its screen went and the new one is not up: nothing to draw on.
        if self.dock_lost() {
            self.dirty = true;
            return;
        }
        // ONE frame per compositor frame. A frame callback is still out: the
        // screen has not shown the last frame yet, so this one waits for the
        // callback (which draws whenever `dirty`). Drawing here anyway — as
        // the colour-match's direct `draw()` calls did — rendered a frame
        // nobody saw AND asked for a second callback, and from then on both
        // callbacks drew, every frame, for as long as the animation ran: the
        // dock measured ~95 fps on a 60 Hz panel, 2 callbacks a vblank (night
        // audit 2026-10-04). Only a `configure` must commit regardless.
        if self.frame_pending && !self.draw_forced {
            self.dirty = true;
            return;
        }
        self.draw_forced = false;

        // F12: on a software (CPU) adapter every frame costs real cores —
        // a minutes-long install animation ran the daemon at 450% CPU in
        // the 8-core VM and throttled its own install's download ~35x. (The
        // GL backend was throttled too while its present blocked the loop,
        // Golem #40, until `NoVblankWait` removed that wait; see
        // renderer.needs_frame_throttle().) While the user is actively interacting it stands down
        // (scrolls/drags stay smooth); ambient animation streams get spaced
        // to SOFTWARE_FRAME_MIN with a timer, and — crucially — that timer
        // rides calloop, so the loop services IPC/input between frames. dt
        // keeps accumulating across skipped frames, so animations advance by
        // real time.
        // GPU pacing (every hardware adapter, see `Renderer::gpu_ready`): the previous
        // frame is still on the GPU, so starting another would only queue in
        // the driver and stall the loop. Look again shortly, on a timer, so
        // IPC and input keep flowing, and draw as soon as the GPU is free.
        if self.renderer.as_ref().is_some_and(|r| !r.gpu_ready()) {
            if self.soft_frame_timer {
                return;
            }
            let timer = calloop::timer::Timer::from_duration(GPU_BUSY_RETRY);
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
            error!("gpu-pace timer failed to arm; drawing without pacing");
        }
        if self.renderer.as_ref().is_some_and(|r| r.needs_frame_throttle())
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

        let _perf = crate::perf::DOCK_DRAW.time();
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.last_frame = Some(now);

        // The open card fits the Apps page in view: its trim eases toward
        // the rows that page leaves empty; out of sight it just snaps, so an
        // open rises straight to the right height.
        // The Controls band eases in and out at the same rate, so the
        // sections make way as one smooth move (no row jump when the row
        // appears).
        // (`fit_animating` keeps frames coming: set after the render, where
        // the frame's `dirty` reset can't swallow it.)
        let mut fit_animating = false;
        let band_want = self.controls_band_target();
        let k = 1.0 - (-dt.min(0.1) * TRIM_RATE).exp();
        let out_of_sight = self.ui.target() != Target::Open && !self.ui.is_animating();
        if !out_of_sight && (band_want - self.controls_band).abs() > 0.25 {
            self.controls_band += (band_want - self.controls_band) * k;
            fit_animating = true;
        } else {
            self.controls_band = band_want;
        }
        // The overflowing pins' window: home when the pointer is off the
        // dock, the far end while it rests on the overflow tile, whole
        // slots once a side-scroll settles; eased snappily.
        let pin_layout = self.layout_at(self.ui.extent());
        match pin_layout.dock_overflow {
            None => {
                self.dock_pin_target = 0.0;
                self.dock_pin_off = 0.0;
            }
            Some(ov) => {
                let on_dock = self.pointer_pos.is_some_and(|p| pin_layout.on_dock(p));
                if !on_dock {
                    self.dock_pin_target = 0.0;
                } else if self.hover == Some(content::Hit::DockIcon(ov.apps_slot)) {
                    self.dock_pin_target = ov.max_off;
                } else if self.dock_hscroll_at.is_some_and(|t| t.elapsed() > Duration::from_millis(160)) {
                    self.dock_pin_target = self.dock_pin_target.round();
                    self.dock_hscroll_at = None;
                }
                self.dock_pin_target = self.dock_pin_target.clamp(0.0, ov.max_off);
                let d = self.dock_pin_target - self.dock_pin_off;
                if d.abs() > 0.002 {
                    // Reduced motion (and a scale dissolve's still) snaps.
                    let k = if animation::reduce_motion() { 1.0 } else { 1.0 - (-dt.min(0.1) * PIN_SLIDE_RATE).exp() };
                    self.dock_pin_off += d * k;
                    fit_animating = true;
                } else {
                    self.dock_pin_off = self.dock_pin_target;
                }
                if self.dock_hscroll_at.is_some() {
                    fit_animating = true;
                }
            }
        }
        // (The deep search's tabs slide between their pages.)
        if self.deep_tabs_ease(dt) {
            fit_animating = true;
        }
        let (trim_want, second_want) = self.apps_trim_target();
        let trim = self.ui.open_trim();
        // Files' second line (a search that left no app) opens and closes
        // with the trim, at its rate: the sections glide, none jumps a row.
        if out_of_sight || animation::reduce_motion() || (second_want - self.files_second).abs() < 0.003 {
            if self.files_second != second_want {
                self.files_second = second_want;
                fit_animating = true;
            }
        } else {
            self.files_second += (second_want - self.files_second) * k;
            fit_animating = true;
        }
        if self.ui.target() != Target::Open && !self.ui.is_animating() {
            if trim != trim_want {
                self.ui.set_open_trim(trim_want);
            }
        } else if (trim_want - trim).abs() > 0.25 {
            // Reduced motion (also what a scale change's dissolve asks for
            // under its still, see `transition.rs`): there at once.
            let k = if animation::reduce_motion() { 1.0 } else { 1.0 - (-dt.min(0.1) * TRIM_RATE).exp() };
            self.ui.set_open_trim(trim + (trim_want - trim) * k);
            fit_animating = true;
        } else if trim != trim_want {
            self.ui.set_open_trim(trim_want);
            fit_animating = true;
        }
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
            // (The keyboard is handed back at the release itself, not here —
            // see `App::begin_keyboard_handback`.)
            // The card just came to rest: re-evaluate the dock colour
            // sample now rather than waiting ≤700 ms for the poll — the
            // card's footprint (whose columns sampling excludes while the
            // card is up) just changed shape, so the right sample columns
            // did too.
            self.reeval_dock_bar();
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
        // The compact search pill's hover eases in and out like a settings
        // pill's (panel::LIFT_RATE).
        let lift_target = if self.hover == Some(content::Hit::SearchButton) && self.search.expand < 0.5 {
            1.0f32
        } else {
            0.0
        };
        let mut lift_animating = (self.search.lift - lift_target).abs() > 0.002;
        // The Controls row's pills hover the same way.
        let rate = 1.0 - (-dt * crate::panel::LIFT_RATE).exp();
        for (i, l) in self.search.control_lift.iter_mut().enumerate() {
            let want = if self.hover == Some(content::Hit::Control(i)) { 1.0 } else { 0.0 };
            if (*l - want).abs() > 0.002 {
                *l += (want - *l) * rate;
                lift_animating = true;
            } else {
                *l = want;
            }
        }
        // An asking Uninstall the keys and the pointer have left is put back.
        self.actions_settle();
        self.search.lift = if lift_animating {
            self.search.lift + (lift_target - self.search.lift) * (1.0 - (-dt * crate::panel::LIFT_RATE).exp())
        } else {
            lift_target
        };

        // Smooth-scroll each section and the open box — one Pager slide.
        let mut scroll_animating = false;
        for sec in &mut self.scroll.per {
            scroll_animating |= sec.ease(dt);
        }
        scroll_animating |= self.box_pager.ease(dt);
        // (A lone app's recents slide between their pages the same way.)
        scroll_animating |= self.recents_slide(dt);

        // Jelly edge: agua_breath is the same spring impulse as agua_card but
        // softer (lower k, c) so it rings longer after the box opens.
        // Perfectly in sync — same stretch_target, just slower to settle.
        let breath_offset = if self.ui.target() == crate::state::Target::Open {
            (self.agua_breath.pos - 1.0) * 40.0
        } else {
            0.0
        };

        self.dirty = false;

        // The frame request is made by the renderer right before it presents
        // (`before_present` below): a frame that changes nothing makes none.

        let bounce = self.bounce_offset();
        let dock_hop = self.dock_hop_offset();
        let layout = self.current_layout();
        // The dock plate grows to wrap minimized tiles; the surface is full
        // width, so the pointer input region must follow the plate's width
        // (else clicks on a newly-grown part fall through, or the whole
        // bottom strip swallows clicks when the plate is small). This changes
        // only when a tile is added/removed — discrete, not per-frame — so
        // re-syncing on change (the region setter no-ops when unchanged)
        // costs nothing during animation.
        if (layout.dock_basin_w - self.dock_basin_w).abs() > 0.5 {
            self.dock_basin_w = layout.dock_basin_w;
            self.sync_input_region();
            // The other screens' edges are as long as it, at rest.
            self.resize_edges();
        }

        // The search caret anchors to the query's shaped width.
        // The search pill is one more OPTIONS pill: the bar pill's size and
        // type at the bar's scale.
        let pill_scale = self.options_scale();
        let search_font = crate::options::FONT_PX * pill_scale;
        let (query_px, search_label_px) = self
            .renderer
            .as_mut()
            .map(|r| {
                (
                    r.measure_text(&self.search.query, search_font, crate::options::TEXT_FONT),
                    r.measure_text(crate::i18n::tr("Search"), search_font, crate::options::TEXT_FONT),
                )
            })
            .unwrap_or((0.0, 0.0));
        // The deep search's chip and placeholder in the pill, measured; its
        // tabs' words and the ones lit.
        let deep_chip = self.deep_chip_text();
        let deep_tab_words: Vec<String> = self.search.deep.as_ref().map(|d| d.tags.iter().map(deep_search::Tag::label).collect()).unwrap_or_default();
        let deep_lit = self.deep_lit();
        let (deep_chip_w, deep_placeholder_w) = match (deep_chip.as_deref(), self.renderer.as_mut()) {
            (Some(chip), Some(r)) => (
                r.measure_text(chip, search_font, crate::options::TEXT_FONT),
                r.measure_text(crate::i18n::tr("Search files"), search_font, crate::options::TEXT_FONT),
            ),
            _ => (0.0, 0.0),
        };
        let search_pill = content::SearchPill {
            scale: pill_scale,
            h: self.options_pill_h(),
            label_px: search_label_px,
            lift: self.search.lift,
        };
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
        // NOT one-shot: the hold-end scan can run before the switch's async
        // user activation has materialized the new `.desktop` (slow-disk
        // ASUS: home-manager lagged the apply by ~90 s), so while the tile
        // stays unresolved the rescan re-arms every RESOLVE_RESCAN_RETRY.
        // Resolution removes the tile, which is what stops the retries.
        let mut fill_done = false;
        for p in &mut self.pending_installs {
            if p.completed_at
                .is_some_and(|c| c.elapsed() >= crate::install::INSTALL_HOLD)
                && p.last_rescan
                    .is_none_or(|t| t.elapsed() >= crate::install::RESOLVE_RESCAN_RETRY)
            {
                p.last_rescan = Some(Instant::now());
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

        // Dock magnification, temporally smoothed: the pointer only sets
        // per-slot *targets* (the same cosine falloff as ever); the drawn
        // scale eases toward its target — fast while growing so the crest
        // stays tight under the cursor, gentler while shrinking so icons
        // melt back instead of snapping. Entering, sweeping, and leaving
        // the dock all become one continuous motion.
        let mag_layout = self.current_layout();
        let n_dock = mag_layout.dock_slots.len();
        if self.dock_mag.len() != n_dock {
            self.dock_mag = vec![1.0; n_dock];
        }
        let icon_scale = self.icon_scale();
        let mut mag_settling = false;
        for (i, slot) in mag_layout.dock_slots.iter().enumerate() {
            let target = if mag_layout.sideways() {
                content::dock_mag_target_side(*slot, mag_layout.dock_band, mag_pointer, icon_scale, self.mag_amount)
            } else {
                content::dock_mag_target(*slot, mag_layout.dock_hit_bottom, mag_pointer, icon_scale, self.mag_amount)
            };
            let rate = if target > self.dock_mag[i] {
                content::DOCK_MAG_GROW_RATE
            } else {
                content::DOCK_MAG_RELEASE_RATE
            };
            let (v, moving) = crate::animation::ease_toward(
                self.dock_mag[i],
                target,
                dt,
                rate,
                content::DOCK_MAG_SNAP,
            );
            self.dock_mag[i] = v;
            mag_settling |= moving;
        }
        let dock_mag = self.dock_mag.clone();

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
                self.group_origin_vp_y = None;
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
        let group_expand = self.group_expand();
        // Ctrl +/−/0 are a dock stack's for as long as one is open.
        self.sync_zoom_keys();

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
                pos: if drag.from_dock && layout.sideways() {
                    // (Beside a side dock: held within the card.)
                    (
                        drag.pos.0.clamp(layout.card_x, layout.card_x + layout.card_w),
                        drag.pos.1.clamp(layout.card_top, layout.card_top + layout.card_h),
                    )
                } else if drag.from_dock {
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
        // A desktop icon in hand arms it too: the dock came up for that
        // drag so the bin would be there (`desktop_lift`).
        let desktop_drag = self.desktop_drag_dock_pos();
        let trash_react_target = desktop_drag.is_some()
            || self.gesture.dragging.as_ref().is_some_and(|d| {
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
            && (desktop_drag.is_some_and(|p| self.dropped_on_trash(&layout, p))
                || self.gesture.dragging.as_ref().is_some_and(|d| {
                    self.dock_fold_target(&layout, d.pos)
                        .and_then(|slot| self.dock_order.get(slot).copied())
                        .and_then(|idx| self.entries.get(idx))
                        .is_some_and(|e| groups::is_trash(&e.id))
                }));
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
        // The dock bin's states, from anywhere in the shell: something of
        // ours in hand anywhere (an app, a desktop icon, a file window's
        // things), over the bin, the pointer on it, a file window in it.
        let throwable = self.desktop_drag_throwable();
        let bin_dragging = throwable && (self.desktop.drag.is_some() || self.filer.dragging() || trash_react_target);
        let on_bin = matches!(self.hover, Some(content::Hit::DockIcon(slot))
            if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| groups::is_trash(&e.id)));
        let bin_now = crate::bin_motion::Inputs {
            // (Animated icons off: the bin still opens for what is carried to it.)
            hover: on_bin && !bin_dragging && self.icons_move(),
            dragging: bin_dragging,
            over: over_trash && throwable,
            open: self.filer.showing_bin(),
        };
        if self.bin_motion.step(bin_now, dt) {
            self.dirty = true;
        }
        // Golem Files' launcher under the pointer (no drag): its folders out.
        let on_files = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::filer::APP_ID));
        if self.files_hover.step(on_files, dt) {
            self.dirty = true;
        }
        // The apps button: its pages turn under the pointer, its squares go
        // down while the grid is open (a frame for each place, its layer).
        let on_apps = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == apps::APPS_GRID_ID));
        let grid_open = self.icons_move() && self.ui.target() == crate::state::Target::Open && !self.control_panel;
        // (Holding pins, the hover spits them first; the pages after.)
        let spat = self
            .layout_at(self.ui.extent())
            .dock_overflow
            .is_none_or(|o| self.dock_pin_off >= o.max_off - 0.02);
        if self.apps_motion.step(on_apps, grid_open, spat, dt) {
            self.dirty = true;
            // (Every entry that is the button: the dock's and the grid's.)
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == apps::APPS_GRID_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            let (pages, down) = self.apps_motion.place();
            if let (Some(chain), Some(renderer)) = (apps::apps_moved(pages, down), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // Seam's compass: the pointer coming on it swings its needle (the
        // plate the other way), a click turns it round; each pose's frame
        // put in its layer.
        let on_seam = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::compass_icon::SEAM_ID));
        if self.compass.step(on_seam, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::compass_icon::SEAM_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            let (needle, plate) = self.compass.pose();
            if let (Some(chain), Some(renderer)) = (apps::compass_at(needle, plate), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // WhatsApp: the pointer on it shakes the phone and turns the plate
        // (held), a click turns both round; each pose's frame in its layer.
        let on_whatsapp = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::whatsapp_icon::WHATSAPP_ID));
        if self.whatsapp.step(on_whatsapp, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::whatsapp_icon::WHATSAPP_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            let (plate, phone) = self.whatsapp.pose();
            if let (Some(chain), Some(renderer)) = (apps::whatsapp_at(plate, phone), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // YouTube: a seek bar and a turning play under the pointer, a pause
        // swelling away on a click; each pose drawn as it comes.
        let on_youtube = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::youtube_icon::YOUTUBE_ID));
        if self.youtube.step(on_youtube, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::youtube_icon::YOUTUBE_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            if let (Some(chain), Some(renderer)) = (apps::youtube_at(&self.youtube.pose()), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // YouTube Music: the play swallows the ring under the pointer and
        // spits it out on a click; each pose drawn as it comes.
        let on_ytmusic = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::ytmusic_icon::YTMUSIC_ID));
        if self.ytmusic.step(on_ytmusic, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::ytmusic_icon::YTMUSIC_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            if let (Some(chain), Some(renderer)) = (apps::ytmusic_at(&self.ytmusic.pose()), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // Discord: Clyde tips and winks under the pointer, drops wide-eyed
        // and blinks on a click; each pose drawn as it comes.
        let on_discord = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::discord_icon::DISCORD_ID));
        if self.discord.step(on_discord, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::discord_icon::DISCORD_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            if let (Some(chain), Some(renderer)) = (apps::discord_at(&self.discord.pose()), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // Spotify: the lines swing round under the pointer, press and jump on
        // a click; each pose drawn as it comes.
        let on_spotify = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::spotify_icon::SPOTIFY_ID));
        if self.spotify.step(on_spotify, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::spotify_icon::SPOTIFY_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            if let (Some(chain), Some(renderer)) = (apps::spotify_at(&self.spotify.pose()), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // Google: the bar sweeps the G away and back under the pointer, the
        // G spins on a click; each pose drawn as it comes.
        let on_google = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::google_icon::GOOGLE_ID));
        if self.google.step(on_google, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::google_icon::GOOGLE_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            if let (Some(chain), Some(renderer)) = (apps::google_at(&self.google.pose()), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // Chrome: the iris opens under the pointer, shuts and reopens on a
        // click; each opening drawn as it comes.
        let on_chrome = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == crate::chrome_icon::CHROME_ID));
        if self.chrome.step(on_chrome, dt) {
            self.dirty = true;
            let layers: Vec<u32> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id == crate::chrome_icon::CHROME_ID)
                .filter_map(|(i, _)| self.icon_layers.get(i).copied())
                .collect();
            if let (Some(chain), Some(renderer)) = (apps::chrome_at(self.chrome.pose(), self.chrome.spin()), self.renderer.as_mut()) {
                for layer in layers {
                    renderer.update_icon_layer(layer, &chain);
                }
            }
        }
        // The control panel's gears: they turn under the pointer, the frame
        // for each angle put in its layer.
        let on_gears = self.icons_move() && self.gesture.dragging.is_none()
            && matches!(self.hover, Some(content::Hit::DockIcon(slot))
                if self.dock_order.get(slot).and_then(|&i| self.entries.get(i)).is_some_and(|e| e.id == apps::CONTROL_PANEL_ID));
        if self.gears_spin.step(on_gears, dt) {
            self.dirty = true;
            let layer = self
                .entries
                .iter()
                .position(|e| e.id == apps::CONTROL_PANEL_ID)
                .and_then(|i| self.icon_layers.get(i).copied());
            if let (Some(layer), Some(chain), Some(renderer)) =
                (layer, apps::gears_turned(self.gears_spin.angle()), self.renderer.as_mut())
            {
                renderer.update_icon_layer(layer, &chain);
            }
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
        let (box_cap, box_pages) = self.box_pages();
        let (open_box_members, open_box_slots): (Vec<usize>, Vec<usize>) = self
            .stack_members()
            .map(|members| {
                // All members with their display slots: their order, the
                // box's line filling before the next page (the box
                // overlay pages them via box_scroll).
                members
                    .iter()
                    .enumerate()
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
                    pages::shifted_slot(s, box_origin, g, box_cap) as f32
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
        let open_box_pages = if self.stack_members().is_some() {
            (self.box_page.min(box_pages - 1), box_pages)
        } else {
            (0, 1)
        };
        // Per-slot running flags for the macOS indicator dot.
        // (How many windows each has: a dot each.)
        let dock_running: Vec<usize> = self
            .dock_order
            .iter()
            .map(|e| self.running.get(e).map_or(0, Vec::len))
            .collect();
        // Corner app-icon badge per minimized tile (dock-tail order); already
        // resolved to a texture layer in `recompute_dock_order`.
        let min_badges: Vec<Option<u32>> = self.minimized.iter().map(|w| w.badge_layer).collect();
        // Aspect per minimized tile, so the draw box-fits each tile exactly
        // as layout() sized its slot (a wide window shrinks in height).
        let min_aspects_draw: Vec<f32> = self.minimized.iter().map(|w| w.aspect).collect();
        // Eased fill/ink/wash/plate: a fresh colour sample fades in instead
        // of repainting in one frame — the ink's black↔white flip and the
        // icon plates' polarity crossfade too. Keep frames coming while
        // anything settles.
        let paint = self.dock_surface_eased(dt);
        let (dock_bg, dock_ink, dock_highlight, dock_tone, dock_seen, dock_open_alpha) = (paint.fill, paint.ink, paint.wash, paint.tone, paint.seen, paint.open_alpha);
        // The control panel's field moves (orbits, layer moves, the open
        // setting), so frames keep coming while the card is up as the panel;
        // sinking with the card, it is drawn still.
        // Apps ↔ settings inside the open card: a snappy ease-out swap. With
        // the card not up there is nothing to swap, so it snaps.
        let want = if self.control_panel { 1.0 } else { 0.0 };
        if self.ui.target() == Target::Open {
            self.panel_mix += (want - self.panel_mix) * (1.0 - (-dt * 22.0).exp());
            if (want - self.panel_mix).abs() < 0.002 {
                self.panel_mix = want;
            } else {
                self.dirty = true;
            }
        } else {
            self.panel_mix = want;
        }
        // The panel starts afresh on each visit, reset while it is out of
        // sight: pushed out behind the apps, or the card fully closed.
        if self.panel_reset_due
            && (self.panel_mix <= 0.001 || (self.ui.target() != Target::Open && !self.ui.is_animating()))
        {
            self.panel.reset();
            self.panel_reset_due = false;
        }
        // (The same for a setting open on the control panel itself.)
        // (The wallpapers' tiles carry `content::WALL`: the card grades them.)
        let panel_draw_images = |app: &mut Self, draw: &crate::panel::PanelDraw| -> Vec<(content::Rect, u32, f32)> {
            draw.images
                .clone()
                .into_iter()
                .filter_map(|(r, file)| app.picture_layer(&file).map(|(l, a)| (r, wall_layer(l, &file), a)))
                .collect()
        };
        // A dragged level that nothing holds any more (the pointer went up
        // somewhere unseen) is let go.
        if crate::palette::wall_live() && self.panel.knob_held().is_none() && self.quick.knob_held().is_none() {
            crate::palette::set_wall_live(None);
        }
        // (Set when this frame is drawn for the panel's orbit alone: the
        // glass's light then stands still — `Renderer::hold_lights`.)
        let mut orbit_only = false;
        let panel_draw = if self.control_panel || self.panel_mix > 0.001 {
            let open = self.ui.target() == Target::Open;
            let bright = dock_ink[0] + dock_ink[1] + dock_ink[2] < 1.5;
            let paint = crate::panel::PanelPaint { ink: dock_ink, bright };
            let draw = self.panel_frame(&layout, if open { dt } else { 0.0 }, paint);
            // Frames keep coming while the panel is up — but when nothing of
            // it moves except the pills' orbit (a pixel round their spots, a
            // turn in 12–18 s), about 30 a second, not one at every refresh
            // of the screen: the panel sitting open cost 19 % of a core on
            // the dev box's 165 Hz screen, and the MacBook's 12 % with the
            // compositor's (2026-10-22). Anything else moving, at full rate.
            if open {
                let calm = self.panel.calm() && self.panel_swap_t >= 1.0;
                if calm {
                    self.orbit_soon();
                } else {
                    self.dirty = true;
                }
                orbit_only = calm;
            }
            Some(draw)
        } else {
            None
        };
        let panel_images = panel_draw.as_ref().map(|d| panel_draw_images(self, d)).unwrap_or_default();
        if paint.moving {
            self.dirty = true;
        }
        // Plates only when the theme wants them at all — `icon_plate` off
        // means raw un-normalized tiles, which a plate would look wrong on.
        let icon_plate = if self.config.theme.icon_plate {
            paint.plate
        } else {
            content::NO_PLATE
        };
        // FAST LAUNCH: the card and its icons (their motion lives in
        // `fast_launch.rs`).
        let fast_geo = self.fast_geometry();
        let (fast_typed, fast_rest) = if self.fast.visible() { self.fast_words() } else { Default::default() };
        let (fast_typed_w, fast_rest_w) = self
            .renderer
            .as_mut()
            .map(|r| {
                let m = |t: &str| r.measure_text(t, fast_geo.font_px, crate::options::TEXT_FONT);
                (m(&fast_typed), m(&fast_rest))
            })
            .unwrap_or((0.0, 0.0));
        // The card grows with the name (plus a margin); closing, it keeps
        // the width it had.
        if self.fast.open {
            let need = fast_typed_w + fast_rest_w + 2.0 * fast_geo.word_margin + fast_geo.word_margin_extra;
            self.fast.wide_to = (need - fast_geo.card.w).max(0.0);
        }
        if self.fast.step(dt) {
            fit_animating = true;
        }
        let fast_view = self.fast.view(&fast_geo);
        // QUICK CONTROLS: the same card with the settings' round pills in the
        // icons' places; and, once one is opened, its box instead of the card.
        let (fast_pill_card, fast_pills) = if self.fast.controls && self.fast.visible() && self.fast.boxed.is_none() {
            self.fast_pills(&fast_geo)
        } else {
            (fast_view.card, Vec::new())
        };
        let (quick_bg, quick_ink, quick_moving) = self.quick_colours(dt, dock_bg, dock_ink);
        let quick = {
            let bright = quick_ink[0] + quick_ink[1] + quick_ink[2] < 1.5;
            self.quick_frame(dt, crate::panel::PanelPaint { ink: quick_ink, bright })
        };
        if quick.is_some() || quick_moving {
            fit_animating = true;
        }
        // Its pictures (the wallpapers' previews), each with its texture
        // once the thumbnailer has it.
        let quick_images: Vec<(content::Rect, u32, f32)> = quick
            .as_ref()
            .map(|(_, _, _, draw)| draw.images.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(r, file)| {
                // What moves, moves here too (Max, 2026-10-13: "mostrar un
                // prev de las imágenes animadas en la caja") — while the
                // box is the one in use, not on its way out.
                if self.fast.boxed.is_some() && crate::controls::wallpaper_moves(&file) {
                    self.picture_play(&file);
                }
                self.picture_layer(&file).map(|(layer, shape)| (r, wall_layer(layer, &file), shape))
            })
            .collect();
        // A picture with no preview (yet, or — a video where none can be
        // made — ever) shows its name instead.
        let quick_names: Vec<(content::Rect, String)> = quick
            .as_ref()
            .map(|(_, _, _, draw)| draw.images.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, file)| !self.has_picture(file))
            .map(|(r, file)| {
                let name = std::path::Path::new(&file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                (r, name)
            })
            .collect();
        let quick_draw = quick.as_ref().map(|(rect, a, scale, draw)| content::QuickDraw {
            rect: *rect,
            a: *a,
            scale: *scale,
            draw,
            images: &quick_images,
            names: &quick_names,
            ink: quick_ink,
            bg: quick_bg,
        });
        let fast_draw = (self.fast.visible() && self.fast.boxed.is_none()).then_some(content::FastDraw {
            card: if self.fast.controls { fast_pill_card } else { fast_view.card },
            radius: fast_geo.radius,
            a: fast_view.card_a,
            icons: if self.fast.controls { &[] } else { &fast_view.icons },
            pills: &fast_pills,
            typed: &fast_typed,
            rest: &fast_rest,
            typed_w: fast_typed_w,
            rest_w: fast_rest_w,
            // The card widens evenly: the word sits left of its middle by
            // half the right margin's extra, as far as the card has grown.
            text_box: {
                let shift = fast_geo.word_margin_extra.min(self.fast.wide) / 2.0;
                content::Rect::new(fast_geo.text.x - shift, fast_geo.text.y, fast_geo.text.w, fast_geo.text.h)
            },
            font_px: fast_geo.font_px,
            line_px: fast_geo.line_px,
            ink: self.fast_backdrop.map(crate::options::ink_on).unwrap_or(dock_ink),
        });
        // The lone app's action cells (`app_actions.rs`).
        let (act_cells, act_asking, act_wide) = self.action_draw();
        let names = self.fitted_names(&layout);
        let recents = self.recents_draw();
        let scene = content::scene(
            &self.config,
            self.icon_scale(),
            &layout,
            &self.entries,
            &self.search.visible,
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            &content::FrameInput {
                names: Some(&names),
                recent_pages: self.recent_pages(),
                recents: recents.as_ref(),
                // Suppress hover highlight and magnification while dragging.
                hover: if drag_frame.is_none() {
                    self.hover
                } else {
                    None
                },
                query_px,
                search_pill,
                fast: fast_draw,
                quick: quick_draw,
                controls: &self.search.controls,
                control_lift: &self.search.control_lift,
                act_cells: &act_cells,
                act_asking,
                act_wide,
                recent_title: self.search.actions.title.as_deref(),
                deep: self.search.deep.as_ref().map(|d| content::DeepDraw {
                    titles: [d.shelves[0].title.as_str(), d.shelves[1].title.as_str(), d.shelves[2].title.as_str()],
                    hidden: [false, d.shelves[1].hidden, d.shelves[2].hidden],
                    subs: &d.subs,
                    marked: &d.marked_cells,
                    tabs: &deep_tab_words,
                    lit: &deep_lit,
                    chip: deep_chip.as_deref().unwrap_or(""),
                    chip_w: deep_chip_w,
                    placeholder_w: deep_placeholder_w,
                }),
                stretch: self.agua_icons.pos,
                dock_tooltip: if drag_frame.is_none() {
                    self.dock_tooltip()
                } else {
                    None
                },
                // (Everything on the Shell's glass: its Transparency past as made.)
                alpha: self.ui.alpha() * crate::options::shell_content(),
                pointer: mag_pointer,
                mag_amount: self.mag_amount,
                dock_mag: &dock_mag,
                plate: icon_plate,
                bounce,
                dock_hop,
                query: &self.search.query,
                selected: self.search.selected.and_then(|i| self.flat_to_pos(i)),
                selected_soft: !self.search.keyed,
                search_expand: self.search.expand,
                placeholders: &self.placeholders,
                kinds: &self.kinds,
                layers: &self.icon_layers,
                dock_order: &self.dock_order,
                dock_running: &dock_running,
                dock_divider: self.dock_divider,
                min_badges: &min_badges,
                min_aspects: &min_aspects_draw,
                drag: drag_frame,
                trash_react: self.trash_react,
                trash_hover: self.trash_hover,
                bin: self.bin_motion.pose(),
                files_fig: self.files_layer.map(|l| (l, self.files_hover.value())),
                files_wave: self.files_hover.lifts(),
                apps_beat: self.apps_motion.scale(),
                apps_stencil: self.apps_stencil,
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
                // (Its tile moves up with its row where the grid is full.)
                group_origin: self.box_origin(&layout).map(|(x, y)| {
                    (x, y - layout.apps_part.filter(|p| p.row >= 0).map_or(0.0, |p| p.up))
                }),
                open_box_members: &open_box_members,
                panel: self.panel_mix >= 0.999,
                panel_mix: self.panel_mix,
                panel_draw: panel_draw.as_ref(),
                panel_images: &panel_images,
                open_box_disp: &open_box_disp,
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
                dock_bg,
                dock_ink,
                dock_highlight,
                dock_tone,
                dock_seen,
                dock_open_alpha,
            },
        );
        let thumb_base = self.thumb_layer_base();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.hold_lights(orbit_only);
        let (layer, qh, frame_pending) = (&self.layer, &self.qh, &mut self.frame_pending);
        let unchanged = match renderer.render(
            &scene,
            dock_ink,
            self.pointer_pos,
            self.config.theme.icon_squircle,
            thumb_base,
            self.dock_visible.as_mut(),
            // Never a second callback while one is out (see the top of `draw`).
            &mut || {
                if !*frame_pending {
                    let wl_surface = layer.wl_surface();
                    wl_surface.frame(qh, wl_surface.clone());
                    *frame_pending = true;
                }
            },
        ) {
            Ok(crate::renderer::Frame::Unchanged) => true,
            Ok(crate::renderer::Frame::Presented) => false,
            Err(e) => {
                error!("render failed: {e:#}");
                false
            }
        };
        if (search_animating && self.search.expand != search_target) || lift_animating || fit_animating {
            self.dirty = true;
        }
        if scroll_animating
            && ((self.ui.target() == Target::Open
                && self.scroll.per.iter().any(pager::Pager::is_settling))
                // A box page-slide keeps animating whatever the card state
                // (a dock stack lives in the Dock state, not Open).
                || (self.stack_open() && self.box_pager.is_settling())
                // …and a lone app's recents' slide between their pages.
                || (self.ui.target() == Target::Open && self.search.recent_pager.is_settling()))
        {
            self.dirty = true;
        }
        // AGUA: the water keeps sloshing briefly after the card lands, and
        // the dock magnification keeps easing toward its targets — keep
        // frames coming until both rest.
        if stretch_active || mag_settling {
            self.dirty = true;
        }
        // Pending jelly impulses waiting on their delay timers.
        if self.jelly.has_pending() || self.box_jelly.has_pending() {
            self.dirty = true;
        }
        // Nothing presented → no frame callback is coming for this frame. An
        // animation still in flight ticks on a short timer instead; a bare
        // commit asking for a callback is not answered by a compositor that
        // damages only what a commit changed (see `Renderer::render`).
        if unchanged && (self.ui.is_animating() || self.bounce.is_some() || self.dock_hop.is_some() || self.dirty) {
            self.redraw_soon();
        }
    }

    /// Draw again in [`ORBIT_FRAME`]: the control panel's pills' orbit is all
    /// that moves. (The frame callback that comes meanwhile keeps the time
    /// of this frame, so the orbit goes on by the clock: `orbit_tick`.)
    fn orbit_soon(&mut self) {
        if self.orbit_tick {
            return;
        }
        let timer = calloop::timer::Timer::from_duration(ORBIT_FRAME);
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.orbit_tick = false;
                app.dirty = true;
                app.draw();
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if armed {
            self.orbit_tick = true;
        } else {
            self.dirty = true;
        }
    }

    /// Draw again shortly, on a timer: for a frame that presented nothing
    /// (so no frame callback is coming) while an animation is still in flight.
    fn redraw_soon(&mut self) {
        if self.soft_frame_timer {
            return;
        }
        let timer = calloop::timer::Timer::from_duration(UNCHANGED_RETRY);
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
        } else {
            error!("redraw timer failed to arm; the animation waits for input");
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
        let scale = self.surface_scale(crate::fractional::SurfaceKind::Options);
        let (pw, ph) = (
            crate::fractional::physical(width, scale),
            crate::fractional::physical(height, scale),
        );
        if let Some(fs) = &self.options_fscale {
            fs.set_logical_size(width, height);
        }
        if let Some(renderer) = self.options_renderer.as_mut() {
            renderer.set_scale(scale);
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
        // Now the size + renderer exist: re-reserve the exclusive zone at the
        // live scale (covers an output whose size arrived before this
        // configure), measure the pill text (incl. the notif rows, in case
        // notifications hydrated before this first configure), set the input
        // region, and draw.
        self.sync_options_zone();
        self.measure_options_text();
        self.task_pill_remeasure();
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
        // (The phone pill's box holds the one menu: placed as the box now
        // stands, before the frame.)
        self.phone_menu_sync();
        let (w, h) = self.options_size;
        // (Nor while its screen has gone and it is being made again.)
        if w == 0 || h == 0 || self.part_lost(crate::shell_screens::Part::Options) {
            return;
        }
        // ONE render per compositor frame, as the dock. The OPTION
        // animations tick on 8 ms timers and every state change calls this
        // directly (67 call sites): on a 60 Hz panel that was 125 renders a
        // second, of which the compositor showed 60 — the newest at each
        // vblank. While the last frame's callback is still out, a draw only
        // marks the surface dirty; the callback renders the newest state.
        // What reaches the screen is the same frame it showed before.
        let concealed =
            self.options_hidden && self.options_sticky.is_none() && self.options_show.t <= 0.0;
        if self.options_frame_pending {
            let overdue = self
                .options_frame_asked
                .is_none_or(|t| t.elapsed() >= FRAME_OVERDUE);
            if !overdue {
                self.options_dirty = true;
                // The bookkeeping a draw does stays on time.
                if !concealed {
                    self.sync_lead();
                }
                return;
            }
            // The callback is overdue (the output is off, the compositor is
            // stalled): draw anyway, at most once per FRAME_OVERDUE, so
            // nothing ever waits on a callback that is not coming.
            self.options_frame_asked = Some(Instant::now());
        }
        // The frame request is made by the renderer right before it presents
        // (`ask_frame`, below): a frame that changes nothing makes none, so no
        // request is ever left waiting on a bare commit.
        self.options_dirty = false;
        let _perf = crate::perf::OPTIONS_DRAW.time();
        // Concealed in fullscreen: render an empty (transparent) frame so the
        // bar disappears until a deliberate top-edge hold reveals it.
        //
        // Unless a sticky OPTION is standing (`OptionUXRules.md` §4) — then the
        // frame carries exactly two pills, the control that took the bar away
        // and its doorway, and falls through to the ordinary draw below so they
        // get the surface's real styling rather than a special-case painting.
        if concealed {
            if let Some(renderer) = self.options_renderer.as_mut() {
                let scene = content::Scene {
                    alpha: 1.0,
                    ..Default::default()
                };
                let (layer, qh) = (self.options_layer.as_ref(), &self.qh);
                let (pending, asked) =
                    (&mut self.options_frame_pending, &mut self.options_frame_asked);
                let frame = renderer.render(
                    &scene,
                    [0.0; 4],
                    None,
                    0.0,
                    0,
                    self.options_visible.as_mut(),
                    &mut || ask_frame(layer, qh, pending, asked),
                );
                self.options_frame_drawn(frame.ok());
            }
            return;
        }
        // The Leader's displacement is derived from the layout, so it is
        // refreshed here — before anything consumes the pills — and this is
        // where a leader that has left the layout is released
        // (`OptionUXRules.md` §1).
        self.sync_lead();
        let w = w as f32;
        let bar_h = self.options_bar_h();
        // Matched: opaque window colour, extended down over the window's top
        // border (the overhang) to hide the seam. Otherwise the faint
        // transparent strip, drawn only to the bar height.
        // The banner surface (see `options_bar_fill` — the one definition,
        // shared with the layer drawn behind the sunset module). Matched:
        // opaque window colour, extended down over the window's top border (the
        // overhang) to hide the seam. Reduce-transparency / a paused fullscreen
        // bar: the opaque slab. Else: the faint 10% strip the frosted wallpaper
        // reads through.
        let (mut color, matched) = self.options_bar_fill();
        let bottom = if self.options_bar_matched.is_some() {
            bar_h + crate::OPTIONS_OVERHANG as f32
        } else {
            bar_h
        };
        // Bleed the top/left/right edges a couple px past the surface so the
        // SDF anti-aliasing seam falls off-screen instead of showing a 1px
        // wallpaper line where an opaque matched bar meets the screen edges.
        // When matched, draw the fill hard-edged (`glass = -1.0`) so its bottom
        // edge — which meets the window's identical colour — is a crisp cut
        // rather than a gamma-lifted AA seam. See `rounded_rect.wgsl`.
        // The strip itself fades with the bar (`OptionUXRules.md` §3): the
        // surface arrives and leaves on the same tempo its pills move at,
        // instead of being the one thing up here that snaps.
        color[3] *= self.options_show.t;
        // THE BANNER stays as it is (Max, 2026-10-15) whatever the Shell's
        // Transparency: the scene is drawn at `shell_content` for the pills and
        // boxes on it, so the strip is put the other way round.
        color[3] /= crate::options::shell_content();
        let strip = |x: f32, y: f32, w: f32, h: f32| content::RectInst {
            rect: content::Rect::new(x, y, w, h),
            radius: 0.0,
            color,
            glass: if matched { -1.0 } else { 0.0 },
            border: 0.0,
        };
        // THE CARD hung from its pill stands IN this row, on a surface under
        // this one: the strip is not drawn where the card is, or the card
        // was seen through it (Max, 2026-10-12: "todavía veo la capa del
        // banner, que es casi transparente… atrás de la tarjeta"). Left of
        // the card, right of it, and the sliver over its top edge.
        let rects = match self.card_banner_in_bar_at() {
            Some((from, to, top)) => vec![
                strip(-2.0, -2.0, from + 2.0, bottom + 2.0),
                strip(to, -2.0, (w + 2.0 - to).max(0.0), bottom + 2.0),
                strip(from, -2.0, to - from, top + 2.0),
            ],
            None => vec![strip(-2.0, -2.0, w + 4.0, bottom + 2.0)],
        };
        let mut scene = content::Scene {
            alpha: crate::options::shell_content(),
            rects,
            ..Default::default()
        };
        // The context-aware pill modules ride on top of the base fill.
        self.push_options_pills(&mut scene);
        // The banner swells around the sunset module (metaball blister).
        scene.neck = self.module_neck();
        // The sunset settings box content, over the banner blister.
        self.push_module_box(&mut scene);
        // The gear's stats readout, over the pill the layout drew for it.
        self.push_stats_child(&mut scene);
        // A hover tooltip for the icon-only OPTION pills (discoverability).
        self.push_options_tooltip(&mut scene);
        crate::options::shell_halos(&mut scene);
        // Adaptive: black text on a bright matched bar, white on a dark one.
        let text_rgba = self.options_text_color();
        let squircle = self.config.theme.icon_squircle;
        let Some(renderer) = self.options_renderer.as_mut() else {
            return;
        };
        // The bar's pointer feeds the glass exactly as the dock's does — the
        // sunset module wears the dock's liquid-glass material, and its
        // cursor-tracked edge reflection is part of the material. Everything
        // else on this surface is glass: 0.0, so nothing else changes.
        let (layer, qh) = (self.options_layer.as_ref(), &self.qh);
        let (pending, asked) = (&mut self.options_frame_pending, &mut self.options_frame_asked);
        match renderer.render(
            &scene,
            text_rgba,
            self.options_ptr,
            squircle,
            0,
            self.options_visible.as_mut(),
            &mut || ask_frame(layer, qh, pending, asked),
        ) {
            Ok(frame) => self.options_frame_drawn(Some(frame)),
            Err(e) => error!("options render failed: {e:#}"),
        }
    }

    /// What became of the OPTIONS frame.
    ///
    /// A frame that was presented made its frame request in the same commit
    /// (`ask_frame`), and its callback paces the next draw. A frame that
    /// changed NOTHING was not presented, nothing is committed for it, and no
    /// request was made: no callback is waited for, and the OPTION
    /// animations' own timers bring the next draw.
    ///
    /// (It used to be committed on its own, to get the callback. To Hyprland
    /// a commit that asks for a frame is a frame to produce, damage or none —
    /// and a frame produced is what a waiting screen capture is delivered on.
    /// So with a player on the bus, whose position moves every second while
    /// the bar shows the same thing, the bar woke the compositor and the
    /// colour sampler once a second for nothing: 17 full-screen captures in
    /// 15 s on the Acer, round 3.)
    fn options_frame_drawn(&mut self, frame: Option<crate::renderer::Frame>) {
        match frame {
            Some(crate::renderer::Frame::Presented) => {}
            Some(crate::renderer::Frame::Unchanged) => self.options_frame_pending = false,
            // Not drawn: the callback is not coming, and the next draw waits
            // out FRAME_OVERDUE as for any that is late.
            None => {}
        }
    }
}

/// A picture's `layer`, marked [`content::WALL`] when it is the tile of a
/// wallpaper that wears the Colours slider (the card grades it).
fn wall_layer(layer: u32, file: &str) -> u32 {
    if crate::wall_tint::tile_graded(file) { layer | content::WALL } else { layer }
}

/// The OPTIONS bar's frame request, made by the renderer in the commit that
/// presents a frame — never before a render, which may change nothing and
/// present nothing (then the request would wait on a bare commit the
/// compositor need not draw).
fn ask_frame(
    layer: Option<&LayerSurface>,
    qh: &QueueHandle<App>,
    pending: &mut bool,
    asked: &mut Option<Instant>,
) {
    if let Some(layer) = layer {
        let surface = layer.wl_surface();
        surface.frame(qh, surface.clone());
        *pending = true;
        *asked = Some(Instant::now());
    }
}

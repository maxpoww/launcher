//! Card content layout: where the dock icon row and the app-grid cells
//! sit for a given animation extent, pointer hit-testing against them,
//! and assembly of the frame's draw scene. Pure math — no Wayland, no
//! wgpu — so it stays unit-testable and is shared by rendering and
//! input handling.

use waverunner_core::index::AppEntry;
use waverunner_core::Config;

/// Axis-aligned rectangle in surface coordinates (logical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Whether `pos` lies inside the rectangle.
    pub fn contains(&self, (px, py): (f32, f32)) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// One rounded rectangle to fill (card background, hover highlight).
#[derive(Debug, Clone, Copy)]
pub struct RectInst {
    pub rect: Rect,
    pub radius: f32,
    pub color: [f32; 4],
}

/// One icon quad, referencing a texture-array layer.
#[derive(Debug, Clone, Copy)]
pub struct IconInst {
    pub rect: Rect,
    pub layer: u32,
}

/// One text label.
#[derive(Debug, Clone)]
pub struct Label {
    pub text: String,
    /// Anchor: (center-x, top) when `centered`, else (left, top).
    pub pos: (f32, f32),
    /// Maximum text width; longer text is clipped.
    pub max_w: f32,
    /// Font size / line height in px.
    pub font_px: f32,
    pub line_px: f32,
    /// Center horizontally about `pos.0` (app names under icons).
    pub centered: bool,
    /// Draw at reduced opacity (placeholder text).
    pub dim: bool,
    /// Cache the shaped glyphs (stable text like app names); the live
    /// query string changes per keystroke and skips the cache.
    pub cache: bool,
    /// Extra clip rect for labels outside the grid scissor.
    pub clip: Option<Rect>,
}

/// Scrollable grid content, clipped to `clip` by the renderer.
#[derive(Debug, Default)]
pub struct GridContent {
    pub clip: Rect,
    pub rects: Vec<RectInst>,
    pub icons: Vec<IconInst>,
    pub labels: Vec<Label>,
}

/// Everything the renderer draws for one frame, in draw order.
#[derive(Debug, Default)]
pub struct Scene {
    pub alpha: f32,
    /// Unclipped fills: card background first, then dock hover and the
    /// search box.
    pub rects: Vec<RectInst>,
    /// Dock icon quads (unclipped; they live in the card's top sliver).
    pub icons: Vec<IconInst>,
    /// Unclipped labels (search box text), clipped per-label.
    pub labels: Vec<Label>,
    /// App grid, present once the card is risen past the dock band.
    pub grid: Option<GridContent>,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    DockIcon(usize),
    GridCell(usize),
    SearchButton,
}

/// Fixed layout metrics (logical px). Config-independent for now; can
/// move into `[theme]` if tuning is wanted.
const DOCK_ICON: f32 = 40.0;
const DOCK_SLOT: f32 = 44.0;
const DOCK_PAD_X: f32 = 10.0;
const GRID_CELL_W: f32 = 104.0;
pub const GRID_CELL_H: f32 = 96.0;
const GRID_ICON: f32 = 54.0;
const GRID_PAD_X: f32 = 14.0;
const GRID_TOP_GAP: f32 = 8.0;
const GRID_BOTTOM_PAD: f32 = 10.0;
/// Text metrics for the app-name labels.
pub const LABEL_FONT_PX: f32 = 12.0;
pub const LABEL_LINE_PX: f32 = 16.0;
/// Search box: height, inner padding, text metrics.
const SEARCH_H: f32 = 32.0;
/// Width of the collapsed "Filter" button pill.
const SEARCH_BTN_W: f32 = 80.0;
/// Minimum expanded width; grows with content beyond this.
const SEARCH_W_MIN: f32 = 160.0;
const SEARCH_PAD_X: f32 = 14.0;
const SEARCH_GAP: f32 = 10.0;
const SEARCH_FONT_PX: f32 = 15.0;
const SEARCH_LINE_PX: f32 = 20.0;

/// Space kept above the fully risen card so magnified dock icons can
/// swell past the card edge (macOS-style). The wl surface is this much
/// taller than the card + gap; the animation extent never enters it.
pub const MAGNIFY_HEADROOM: f32 = 24.0;
/// Peak scale of a dock icon directly under the cursor.
const DOCK_MAGNIFY: f32 = 1.5;
/// Horizontal falloff radius of dock magnification, in pixels.
const DOCK_MAG_RADIUS: f32 = 120.0;
/// Vertical attenuation radius, measured from the dock band's edges
/// (zero inside the band). Small on purpose: the effect must die out
/// before the first grid row so hovering the grid never stirs the dock.
const DOCK_MAG_VRADIUS: f32 = 28.0;
/// Peak scale of a grid icon under the cursor.
const GRID_MAGNIFY: f32 = 1.22;
/// Radial falloff radius of grid magnification, in pixels.
const GRID_MAG_RADIUS: f32 = 95.0;

/// Cosine ease from 1.0 at `d == 0` to 0.0 at `d >= radius`.
fn falloff(d: f32, radius: f32) -> f32 {
    let t = (d.abs() / radius).min(1.0);
    0.5 * (1.0 + (std::f32::consts::PI * t).cos())
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Truncate a label to fit within `max_w` pixels, appending "…" when
/// the text is longer. Uses an average-character-width estimate (0.52×
/// font size) rather than shaping — close enough for app names.
fn truncate_label(text: &str, max_w: f32, font_px: f32) -> String {
    let avg_char_w = font_px * 0.52;
    let max_chars = (max_w / avg_char_w) as usize;
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", truncated.trim_end())
    }
}

/// Geometry shared by scene assembly and hit-testing.
#[derive(Debug)]
pub struct Layout {
    /// Card top edge in surface coordinates for the current extent.
    pub card_top: f32,
    /// Dock slot rects, one per shown dock icon (entry index == slot
    /// index: the dock shows the first N entries, unaffected by search).
    pub dock_slots: Vec<Rect>,
    /// Grid viewport clipped to an exact multiple of GRID_CELL_H rows.
    pub viewport: Rect,
    /// Left edge of the first grid column (columns are centered).
    pub grid_x0: f32,
    /// Number of grid columns (>= 1).
    pub cols: usize,
    /// Rows visible at once (one page height).
    pub rows_per_page: usize,
    /// Total pages of grid content.
    pub n_pages: usize,
    /// Horizontal scroll offset in pixels (page 0 = 0, page 1 =
    /// viewport.w, …), normalized into [0, n_pages * viewport.w) —
    /// paging wraps cyclically.
    pub scroll: f32,
    /// Total number of grid cells (the *visible*, filtered entries).
    pub cells: usize,
    /// Fully expanded search box (centered, fixed width).
    pub search_box: Rect,
    /// Compact search button (circle, same center-x and y as search_box).
    pub search_btn: Rect,
}

/// Compute the layout for the current animation state.
///
/// `surface` is the full surface size, `extent` the card's rise,
/// `n_entries` the total (unfiltered) entry count for the dock,
/// `n_visible` the filtered count for the grid, `scroll` the unclamped
/// grid offset.
pub fn layout(
    config: &Config,
    surface: (f32, f32),
    extent: f32,
    n_entries: usize,
    n_visible: usize,
    scroll: f32,
) -> Layout {
    let (w, h) = surface;
    let card_top = h - extent;
    let dock_h = config.window.input_bar_height as f32;
    let card_h = h - config.window.bottom_margin as f32 - MAGNIFY_HEADROOM;

    // Dock uses n_entries so search never hides dock icons.
    let max_slots = (((w - 2.0 * DOCK_PAD_X) / DOCK_SLOT).floor() as usize).max(1);
    let n_dock = n_entries.min(max_slots);
    let start_x = (w - n_dock as f32 * DOCK_SLOT) / 2.0;
    let dock_slots = (0..n_dock)
        .map(|i| {
            Rect::new(
                start_x + i as f32 * DOCK_SLOT,
                card_top + (dock_h - DOCK_SLOT).max(0.0) / 2.0,
                DOCK_SLOT,
                DOCK_SLOT.min(dock_h),
            )
        })
        .collect();

    // Compact button and minimum-width search box; scene() stretches the
    // box dynamically with the query so layout just anchors the y position.
    let search_box = Rect::new(
        (w - SEARCH_W_MIN) / 2.0,
        card_top + card_h - GRID_BOTTOM_PAD - SEARCH_H,
        SEARCH_W_MIN,
        SEARCH_H,
    );
    let search_btn = Rect::new(
        (w - SEARCH_BTN_W) / 2.0,
        search_box.y,
        SEARCH_BTN_W,
        SEARCH_H,
    );

    let grid_top = card_top + dock_h + GRID_TOP_GAP;
    let grid_bottom = (search_box.y - SEARCH_GAP).min(h);
    let inner_w = (w - 2.0 * GRID_PAD_X).max(GRID_CELL_W);
    let cols = ((inner_w / GRID_CELL_W).floor() as usize).max(1);
    let grid_x0 = (w - cols as f32 * GRID_CELL_W) / 2.0;
    // Truncate viewport height to a clean multiple of GRID_CELL_H so every
    // page has the same number of full rows.
    let raw_h = (grid_bottom - grid_top).max(0.0);
    let rows_per_page = ((raw_h / GRID_CELL_H).floor() as usize).max(1);
    let viewport_h = rows_per_page as f32 * GRID_CELL_H;
    let viewport = Rect::new(grid_x0, grid_top, cols as f32 * GRID_CELL_W, viewport_h);

    // Horizontal paging: each page is viewport.w wide. Pages wrap
    // cyclically (infinite scroll), so the offset is normalized into
    // [0, n_pages * viewport.w) rather than clamped.
    let cells_per_page = cols * rows_per_page;
    let n_pages = if n_visible == 0 {
        0
    } else {
        n_visible.div_ceil(cells_per_page)
    };
    let total_w = n_pages as f32 * viewport.w;
    let scroll = if total_w > 0.0 {
        scroll.rem_euclid(total_w)
    } else {
        0.0
    };

    Layout {
        card_top,
        dock_slots,
        viewport,
        grid_x0,
        cols,
        rows_per_page,
        n_pages,
        scroll,
        cells: n_visible,
        search_box,
        search_btn,
    }
}

/// Return the horizontal scroll offset (page * viewport.w) that makes
/// `cell` visible, snapping to the page that contains it.
pub fn scroll_to_reveal(layout: &Layout, cell: usize) -> f32 {
    let cells_per_page = (layout.cols * layout.rows_per_page).max(1);
    let page = cell / cells_per_page;
    let max_page = layout.n_pages.saturating_sub(1);
    (page.min(max_page) as f32 * layout.viewport.w).max(0.0)
}

/// Which item (if any) the pointer is over.
///
/// `search_open` controls whether the compact search button is a target
/// (it is only when the search box is collapsed).
pub fn hit_test(layout: &Layout, pos: (f32, f32), search_open: bool) -> Option<Hit> {
    for (i, slot) in layout.dock_slots.iter().enumerate() {
        if slot.contains(pos) {
            return Some(Hit::DockIcon(i));
        }
    }
    if layout.viewport.h > 0.0 {
        if !search_open && layout.search_btn.contains(pos) {
            return Some(Hit::SearchButton);
        }
        if layout.viewport.contains(pos) {
            // Adjust for horizontal page scroll; pages wrap cyclically.
            let adjusted_x = pos.0 - layout.grid_x0 + layout.scroll;
            let page_w = layout.viewport.w.max(1.0);
            let page = (adjusted_x / page_w).floor() as usize % layout.n_pages.max(1);
            let col_f = (adjusted_x - page as f32 * page_w) / GRID_CELL_W;
            let row = ((pos.1 - layout.viewport.y) / GRID_CELL_H).floor() as usize;
            let col = col_f.floor() as usize;
            if col_f >= 0.0 && col < layout.cols && row < layout.rows_per_page {
                let cells_per_page = layout.cols * layout.rows_per_page;
                let i = page * cells_per_page + row * layout.cols + col;
                if i < layout.cells {
                    return Some(Hit::GridCell(i));
                }
            }
        }
    }
    None
}

/// Active drag state forwarded to scene assembly for ghost + indicator.
#[derive(Debug, Clone, Copy)]
pub struct DragFrame {
    /// The entry being dragged (index into the entries slice).
    pub entry_idx: usize,
    /// Current pointer position in surface coordinates.
    pub pos: (f32, f32),
    /// Target dock insertion slot (0 = before first, n = after last)
    /// while hovering the dock band; `None` when outside the dock.
    pub dock_insert: Option<usize>,
}

/// Per-frame dynamic inputs to scene assembly.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInput<'a> {
    /// Item under the pointer (hover highlight).
    pub hover: Option<Hit>,
    /// Card opacity from the animation.
    pub alpha: f32,
    /// Pointer position driving the macOS-style magnification (icons
    /// swell under the cursor with cosine falloff; purely visual,
    /// hit-boxes stay fixed).
    pub pointer: Option<(f32, f32)>,
    /// Launch bounce: (entry index, upward offset in px).
    pub bounce: Option<(usize, f32)>,
    /// Live search query (empty shows the placeholder).
    pub query: &'a str,
    /// Auto-selected grid position (best match while searching).
    pub selected: Option<usize>,
    /// Search box expansion: 0.0 = compact circle button, 1.0 = full pill.
    pub search_expand: f32,
    /// Which entries are using placeholder tiles (no resolved icon file).
    /// Aligned with the `entries` slice passed to `scene()`.
    pub placeholders: &'a [bool],
    /// Dock display order: maps slot position → entry index. Empty slice
    /// renders no dock icons (safe default for tests / pre-load frames).
    pub dock_order: &'a [usize],
    /// Active drag for ghost icon and insertion indicator; `None` at rest.
    pub drag: Option<DragFrame>,
}

/// Assemble the draw scene for one frame.
///
/// `visible` holds indices into `entries`, ranked by the current query
/// — the grid shows exactly these, in order. Dock icons always show
/// the first entries unfiltered.
pub fn scene(
    config: &Config,
    layout: &Layout,
    entries: &[AppEntry],
    visible: &[usize],
    surface: (f32, f32),
    frame: &FrameInput,
) -> Scene {
    let FrameInput {
        hover,
        alpha,
        pointer,
        bounce,
        query,
        selected,
        search_expand,
        placeholders,
        dock_order,
        drag,
    } = *frame;
    let (w, h) = surface;
    let card_h = h - config.window.bottom_margin as f32 - MAGNIFY_HEADROOM;
    let mut scene = Scene {
        alpha,
        ..Default::default()
    };
    let lift = |i: usize| match bounce {
        Some((b, offset)) if b == i => offset,
        _ => 0.0,
    };

    // Card background.
    scene.rects.push(RectInst {
        rect: Rect::new(0.0, layout.card_top, w, card_h),
        radius: config.theme.corner_radius,
        color: config.theme.background_rgba(),
    });

    // Dock row: per-icon magnification scales, then spread visual centers.
    // Hit-boxes stay at fixed slot positions; only drawn positions spread.
    let dock_scales: Vec<f32> = layout
        .dock_slots
        .iter()
        .map(|slot| {
            let cx = slot.x + slot.w / 2.0;
            match pointer {
                Some((px, py)) => {
                    let d_out = (slot.y - py).max(py - (slot.y + slot.h)).max(0.0);
                    let fy = falloff(d_out, DOCK_MAG_VRADIUS);
                    1.0 + (DOCK_MAGNIFY - 1.0) * falloff(px - cx, DOCK_MAG_RADIUS) * fy
                }
                None => 1.0,
            }
        })
        .collect();
    // Each magnified icon widens its visual slot by the extra pixels it
    // grew, pushing neighbours apart. Row stays centered as a whole.
    let dock_vcx: Vec<f32> = {
        let total_vw: f32 = dock_scales
            .iter()
            .map(|&s| DOCK_SLOT + DOCK_ICON * (s - 1.0))
            .sum();
        let mut x = (w - total_vw) / 2.0;
        let mut centers = Vec::with_capacity(dock_scales.len());
        for &s in &dock_scales {
            let vw = DOCK_SLOT + DOCK_ICON * (s - 1.0);
            centers.push(x + vw / 2.0);
            x += vw;
        }
        centers
    };
    // Hover highlight (suppressed while dragging).
    if drag.is_none() {
        if let Some(Hit::DockIcon(i)) = hover {
            if let (Some(slot), Some(&vcx)) = (layout.dock_slots.get(i), dock_vcx.get(i)) {
                scene.rects.push(RectInst {
                    rect: Rect::new(vcx - DOCK_SLOT / 2.0, slot.y, DOCK_SLOT, slot.h),
                    radius: 12.0,
                    color: config.theme.highlight_rgba(),
                });
            }
        }
    }
    // Dock icons: slot index → entry index via dock_order.
    for slot in 0..layout.dock_slots.len() {
        let Some(&entry_idx) = dock_order.get(slot) else {
            break;
        };
        let slot_rect = &layout.dock_slots[slot];
        let vcx = dock_vcx[slot];
        let baseline = slot_rect.y + slot_rect.h;
        let scale = dock_scales[slot];
        let size = (DOCK_ICON * scale).min(slot_rect.h.min(DOCK_SLOT) + MAGNIFY_HEADROOM);
        scene.icons.push(IconInst {
            rect: Rect::new(
                vcx - size / 2.0,
                baseline - size - lift(entry_idx),
                size,
                size,
            ),
            layer: entry_idx as u32,
        });
        if placeholders.get(entry_idx).copied().unwrap_or(false) {
            if let Some(ch) = entries.get(entry_idx).and_then(|e| e.name.chars().next()) {
                let letter: String = ch.to_uppercase().collect();
                let font_px = (size * 0.46).clamp(10.0, 28.0);
                let line_px = font_px * 1.3;
                let icon_center_y = baseline - size / 2.0 - lift(entry_idx);
                scene.labels.push(Label {
                    text: letter,
                    pos: (vcx, icon_center_y - line_px / 2.0),
                    max_w: size,
                    font_px,
                    line_px,
                    centered: true,
                    dim: false,
                    cache: true,
                    clip: None,
                });
            }
        }
    }
    // Drag-and-drop visuals: insertion bar + ghost icon.
    if let Some(df) = drag {
        // Thin vertical bar at the would-be insertion point.
        if let Some(insert_slot) = df.dock_insert {
            if let Some(first) = layout.dock_slots.first() {
                let n = layout.dock_slots.len();
                let bar_x = if insert_slot >= n {
                    layout.dock_slots[n - 1].x + layout.dock_slots[n - 1].w
                } else {
                    layout.dock_slots[insert_slot].x
                };
                let bar_h = DOCK_ICON * 0.85;
                let bar_y = first.y + (first.h - bar_h) / 2.0;
                let hl = config.theme.highlight_rgba();
                scene.rects.push(RectInst {
                    rect: Rect::new(bar_x - 2.0, bar_y, 4.0, bar_h),
                    radius: 2.0,
                    color: [hl[0], hl[1], hl[2], 0.9],
                });
            }
        }
        // Ghost icon following the pointer.
        let size = DOCK_ICON * 1.05;
        let (gx, gy) = df.pos;
        scene.icons.push(IconInst {
            rect: Rect::new(gx - size / 2.0, gy - size / 2.0, size, size),
            layer: df.entry_idx as u32,
        });
    }

    // Search widget: "Filter" pill button that morphs into an expanding
    // search box. search_expand drives the open animation (0→1).
    if layout.viewport.h > 1.0 {
        let btn = layout.search_btn;
        let boxx = layout.search_box;
        let cx = w / 2.0;
        let is_btn_hover = hover == Some(Hit::SearchButton) && search_expand < 0.5;
        let box_color = {
            let hl = config.theme.highlight_rgba();
            let a = if is_btn_hover {
                (hl[3] * 1.0).min(1.0)
            } else {
                (hl[3] * 0.6).min(1.0)
            };
            [hl[0], hl[1], hl[2], a]
        };
        // Expanded width: SEARCH_W_MIN, or wider to snugly fit the query.
        // 0.52 × font_px ≈ average proportional glyph width at this size.
        // +1 for the cursor character shown when query is non-empty.
        let content_w = {
            let chars = if query.is_empty() { 0 } else { query.len() + 1 };
            let text_px = chars as f32 * SEARCH_FONT_PX * 0.52 + 2.0 * SEARCH_PAD_X;
            text_px.max(SEARCH_W_MIN).min(w - 2.0 * GRID_PAD_X)
        };
        let sw = lerp(btn.w, content_w, search_expand);
        let draw_rect = Rect::new(cx - sw / 2.0, boxx.y, sw, SEARCH_H);
        scene.rects.push(RectInst {
            rect: draw_rect,
            radius: SEARCH_H / 2.0,
            color: box_color,
        });

        if search_expand < 0.5 {
            // Compact button: "Search" label.
            scene.labels.push(Label {
                text: "Search".to_string(),
                pos: (cx, boxx.y + (SEARCH_H - SEARCH_LINE_PX) / 2.0),
                max_w: btn.w,
                font_px: SEARCH_FONT_PX,
                line_px: SEARCH_LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                clip: None,
            });
        } else {
            // Expanded: show query text with a static cursor, or dim
            // placeholder when the box is open but nothing typed yet.
            let (text, dim) = if query.is_empty() {
                ("Search".to_string(), true)
            } else {
                (format!("{}│", query), false)
            };
            scene.labels.push(Label {
                text,
                pos: (cx, boxx.y + (SEARCH_H - SEARCH_LINE_PX) / 2.0),
                max_w: sw - 2.0 * SEARCH_PAD_X,
                font_px: SEARCH_FONT_PX,
                line_px: SEARCH_LINE_PX,
                centered: true,
                dim,
                cache: query.is_empty(),
                clip: Some(draw_rect),
            });
        }
    }

    // Grid cells with horizontal paging.
    if layout.viewport.h > 1.0 && !visible.is_empty() {
        let mut grid = GridContent {
            clip: layout.viewport,
            ..Default::default()
        };
        let page_w = layout.viewport.w;
        let cells_per_page = (layout.cols * layout.rows_per_page).max(1);
        let total_w = (layout.n_pages as f32 * page_w).max(1.0);
        // Paging wraps cyclically: place every page at its nearest wrapped
        // offset relative to the viewport and cull the off-screen ones (at
        // most two pages remain visible during a slide).
        for page in 0..layout.n_pages {
            let rel0 = (page as f32 * page_w - layout.scroll).rem_euclid(total_w);
            let rel = if rel0 >= page_w { rel0 - total_w } else { rel0 };
            if rel <= -page_w || rel >= page_w {
                continue;
            }
            for row in 0..layout.rows_per_page {
                for col in 0..layout.cols {
                    let i = page * cells_per_page + row * layout.cols + col;
                    let Some(&entry_idx) = visible.get(i) else {
                        break;
                    };
                    let Some(entry) = entries.get(entry_idx) else {
                        break;
                    };
                    let cell_x = layout.grid_x0 + rel + col as f32 * GRID_CELL_W;
                    let cell_y = layout.viewport.y + row as f32 * GRID_CELL_H;
                    let cell = Rect::new(cell_x, cell_y, GRID_CELL_W, GRID_CELL_H);
                    if selected == Some(i) {
                        let hl = config.theme.highlight_rgba();
                        grid.rects.push(RectInst {
                            rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                            radius: 14.0,
                            color: [hl[0], hl[1], hl[2], (hl[3] * 1.8).min(0.4)],
                        });
                    } else if hover == Some(Hit::GridCell(i)) {
                        grid.rects.push(RectInst {
                            rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                            radius: 14.0,
                            color: config.theme.highlight_rgba(),
                        });
                    }
                    let cx = cell.x + cell.w / 2.0;
                    let icon_cy = cell.y + 12.0 + GRID_ICON / 2.0;
                    let scale = match pointer {
                        Some((px, py)) => {
                            let d = ((px - cx).powi(2) + (py - icon_cy).powi(2)).sqrt();
                            1.0 + (GRID_MAGNIFY - 1.0) * falloff(d, GRID_MAG_RADIUS)
                        }
                        None => 1.0,
                    };
                    let size = GRID_ICON * scale;
                    grid.icons.push(IconInst {
                        rect: Rect::new(
                            cx - size / 2.0,
                            icon_cy - size / 2.0 - lift(entry_idx),
                            size,
                            size,
                        ),
                        layer: entry_idx as u32,
                    });
                    if placeholders.get(entry_idx).copied().unwrap_or(false) {
                        if let Some(ch) = entry.name.chars().next() {
                            let letter: String = ch.to_uppercase().collect();
                            let font_px = (size * 0.46).clamp(10.0, 30.0);
                            let line_px = font_px * 1.3;
                            grid.labels.push(Label {
                                text: letter,
                                pos: (cx, icon_cy - lift(entry_idx) - line_px / 2.0),
                                max_w: size,
                                font_px,
                                line_px,
                                centered: true,
                                dim: false,
                                cache: true,
                                clip: None,
                            });
                        }
                    }
                    grid.labels.push(Label {
                        text: truncate_label(&entry.name, cell.w - 12.0, LABEL_FONT_PX),
                        pos: (cx, cell.y + 12.0 + GRID_ICON + 8.0),
                        max_w: cell.w - 12.0,
                        font_px: LABEL_FONT_PX,
                        line_px: LABEL_LINE_PX,
                        centered: true,
                        dim: false,
                        cache: true,
                        clip: None,
                    });
                }
            }
        }
        scene.grid = Some(grid);

        // Page indicator dots between the grid and the search button.
        if layout.n_pages > 1 {
            let dot_r = 3.0;
            let dot_spacing = 10.0;
            let total_dots_w = layout.n_pages as f32 * dot_spacing - (dot_spacing - dot_r * 2.0);
            let dot_cx = layout.viewport.x + layout.viewport.w / 2.0;
            let dot_y = layout.viewport.y + layout.viewport.h + 10.0;
            let page_frac = layout.scroll / page_w;
            let hl = config.theme.highlight_rgba();
            for p in 0..layout.n_pages {
                let x = dot_cx - total_dots_w / 2.0 + p as f32 * dot_spacing + dot_r;
                // Cyclic distance: the highlight wraps with the pages.
                let raw = (p as f32 - page_frac).abs();
                let dist = raw.min(layout.n_pages as f32 - raw).min(1.0);
                let alpha = 1.0 - dist * 0.72;
                scene.rects.push(RectInst {
                    rect: Rect::new(x - dot_r, dot_y - dot_r, dot_r * 2.0, dot_r * 2.0),
                    radius: dot_r,
                    color: [hl[0], hl[1], hl[2], hl[3] * alpha],
                });
            }
        }
    } else if layout.viewport.h > 1.0 && !query.is_empty() {
        // Search active but nothing matched: show a soft empty state.
        let cx = layout.viewport.x + layout.viewport.w / 2.0;
        let cy = layout.viewport.y + layout.viewport.h / 2.0;
        let mut grid = GridContent {
            clip: layout.viewport,
            ..Default::default()
        };
        grid.labels.push(Label {
            text: "No results".to_string(),
            pos: (cx, cy - LABEL_LINE_PX / 2.0),
            max_w: 200.0,
            font_px: LABEL_FONT_PX + 2.0,
            line_px: LABEL_LINE_PX + 2.0,
            centered: true,
            dim: true,
            cache: true,
            clip: None,
        });
        scene.grid = Some(grid);
    }

    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<AppEntry> {
        (0..n)
            .map(|i| AppEntry {
                id: format!("app{i}"),
                name: format!("App {i}"),
                description: None,
                exec: "true".into(),
                icon: None,
                needs_terminal: false,
            })
            .collect()
    }

    fn vis(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    fn config() -> Config {
        Config::default()
    }

    const SURFACE: (f32, f32) = (720.0, 572.0);

    #[test]
    fn docked_extent_shows_dock_row_only() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 48.0, 20, 20, 0.0);
        assert!(!l.dock_slots.is_empty());
        // Viewport exists geometrically but lies below the surface
        // bottom, so no cells are visible while docked.
        let s = scene(
            &cfg,
            &l,
            &entries(20),
            &vis(20),
            SURFACE,
            &FrameInput {
                alpha: 1.0,
                ..Default::default()
            },
        );
        assert!(s.grid.is_none() || s.grid.as_ref().unwrap().clip.y >= SURFACE.1);
    }

    #[test]
    fn open_extent_shows_grid_and_wraps_scroll() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 40, 40, 1e9);
        assert!(l.cols >= 2, "720px card should fit several columns");
        assert!(l.viewport.h > 100.0);
        // Scroll wraps cyclically into the page strip.
        assert!(l.scroll >= 0.0 && l.scroll < l.n_pages as f32 * l.viewport.w);
        let s = scene(
            &cfg,
            &l,
            &entries(40),
            &vis(40),
            SURFACE,
            &FrameInput {
                alpha: 1.0,
                ..Default::default()
            },
        );
        let grid = s.grid.expect("open card must show the grid");
        assert!(!grid.icons.is_empty());
        assert_eq!(grid.icons.len(), grid.labels.len());
    }

    #[test]
    fn negative_scroll_wraps_to_last_page() {
        let cfg = config();
        let page_w = l_page_w(&cfg);
        // One page-width left of page 0 is the last page's position.
        let l = layout(&cfg, SURFACE, 572.0, 72, 72, -page_w);
        assert!(l.n_pages >= 2, "test needs multiple pages");
        let expect = (l.n_pages - 1) as f32 * page_w;
        assert!(
            (l.scroll - expect).abs() < 0.5,
            "scroll={} expected={}",
            l.scroll,
            expect
        );
    }

    #[test]
    fn hit_test_finds_dock_icon_and_cell() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 10, 10, 0.0);
        let slot = l.dock_slots[0];
        assert_eq!(
            hit_test(&l, (slot.x + 1.0, slot.y + 1.0), false),
            Some(Hit::DockIcon(0))
        );
        // Second cell of the first row.
        let cell1 = (
            l.grid_x0 + GRID_CELL_W * 1.5,
            l.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(hit_test(&l, cell1, false), Some(Hit::GridCell(1)));
        assert_eq!(hit_test(&l, (1.0, 1.0), false), None);
    }

    #[test]
    fn scrolled_hit_test_offsets_cells() {
        let cfg = config();
        // Scroll exactly one page forward.
        let cells_per_page_guess = 6 * 4; // rough: 6 cols × 4 rows
        let l = layout(
            &cfg,
            SURFACE,
            572.0,
            cells_per_page_guess * 3,
            cells_per_page_guess * 3,
            l_page_w(&cfg),
        );
        // Top-left of the visible area is now the first cell of page 1.
        let pos = (l.grid_x0 + 2.0, l.viewport.y + 2.0);
        let cells_per_page = l.cols * l.rows_per_page;
        assert_eq!(
            hit_test(&l, pos, false),
            Some(Hit::GridCell(cells_per_page))
        );
    }

    fn l_page_w(cfg: &Config) -> f32 {
        layout(cfg, SURFACE, 572.0, 24, 24, 0.0).viewport.w
    }

    #[test]
    fn partial_last_row_is_not_hit_beyond_entries() {
        let cfg = config();
        // 7 entries in >= 2 columns: the last row is partial.
        let l = layout(&cfg, SURFACE, 572.0, 7, 7, 0.0);
        let last_row = 7 / l.cols;
        let beyond = 7 % l.cols; // first empty column of the last row
        if beyond > 0 {
            let pos = (
                l.grid_x0 + (beyond as f32 + 0.5) * GRID_CELL_W,
                l.viewport.y + (last_row as f32 + 0.5) * GRID_CELL_H,
            );
            assert_eq!(hit_test(&l, pos, false), None);
        }
    }
}

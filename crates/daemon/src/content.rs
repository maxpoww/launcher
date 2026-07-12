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
    /// Section grids (Apps / Install / Files), each clipped to its own
    /// viewport by the renderer.
    pub grids: Vec<GridContent>,
    /// Topmost icon quads, drawn over everything (the drag ghost —
    /// what's in your hand must never hide behind the grid).
    pub overlay: Vec<IconInst>,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    DockIcon(usize),
    /// A cell in one of the popup sections: (section, cell index into
    /// that section's visible list).
    GridCell(usize, usize),
    SearchButton,
    /// The "‹ Back" button beside the Files title (navigated dirs only).
    FilesBack,
    /// The "‹ Back" button beside the Apps title (open group only).
    AppsBack,
}

/// Popup sections, top to bottom: the app grid, the (future) install
/// row, and the home-folder row. Each pages independently.
pub const SECTION_APPS: usize = 0;
pub const SECTION_INSTALL: usize = 1;
pub const SECTION_FILES: usize = 2;
pub const N_SECTIONS: usize = 3;
const SECTION_TITLES: [&str; N_SECTIONS] = ["Apps", "Install", "Files"];
/// Grid rows per section (Apps is a cap; it shrinks on short cards).
const SECTION_ROWS: [usize; N_SECTIONS] = [3, 1, 1];
/// Height of a section's title line above its grid.
const SECTION_TITLE_H: f32 = 22.0;
/// Vertical gap beneath each section (page dots live here).
const SECTION_GAP: f32 = 14.0;
/// Size of the "‹ Back" pill beside the Files title.
const BACK_W: f32 = 52.0;
const BACK_H: f32 = 18.0;
/// Columns of the compact grid an open app group shows (3×3 pages).
const GROUP_COLS: usize = 3;

/// Fixed layout metrics (logical px). Config-independent for now; can
/// move into `[theme]` if tuning is wanted.
pub const DOCK_ICON: f32 = 40.0;
const DOCK_SLOT: f32 = 44.0;
const DOCK_PAD_X: f32 = 10.0;
pub const GRID_CELL_W: f32 = 104.0;
pub const GRID_CELL_H: f32 = 96.0;
pub const GRID_ICON: f32 = 54.0;
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
/// Transparent margin around the card, on each side and above it: the
/// wl surface is this much wider/taller than the card so a dragged
/// icon (drawn topmost, unclipped within the surface) has room to roam
/// past the card edges before the surface boundary clips it. The card
/// itself stays centered and unchanged.
pub const DRAG_MARGIN_X: f32 = 80.0;
pub const DRAG_MARGIN_TOP: f32 = 56.0;
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

/// One section's geometry: title anchor plus a grid viewport with its
/// own cyclic horizontal paging.
#[derive(Debug)]
pub struct SectionLayout {
    /// Title anchor: (left, top).
    pub title_pos: (f32, f32),
    /// Grid viewport (exactly `rows` × GRID_CELL_H tall).
    pub viewport: Rect,
    /// Number of grid columns (>= 1).
    pub cols: usize,
    /// Rows visible at once (one page height).
    pub rows: usize,
    /// Total pages of grid content.
    pub n_pages: usize,
    /// Horizontal scroll offset in pixels (page 0 = 0, page 1 =
    /// viewport.w, …), normalized into [0, n_pages * viewport.w) —
    /// paging wraps cyclically.
    pub scroll: f32,
    /// Total number of grid cells (the *visible*, filtered entries).
    pub cells: usize,
}

/// Geometry shared by scene assembly and hit-testing.
#[derive(Debug)]
pub struct Layout {
    /// Card top edge in surface coordinates for the current extent.
    pub card_top: f32,
    /// Dock slot rects, one per shown dock icon (entry index == slot
    /// index: the dock shows the first N entries, unaffected by search).
    pub dock_slots: Vec<Rect>,
    /// The popup sections, top to bottom (Apps, Install, Files).
    pub sections: [SectionLayout; N_SECTIONS],
    /// Fully expanded search box (centered, fixed width).
    pub search_box: Rect,
    /// Compact search button (circle, same center-x and y as search_box).
    pub search_btn: Rect,
    /// "‹ Back" button beside the Files title; present only while the
    /// Files section is navigated into a directory.
    pub files_back: Option<Rect>,
    /// "‹ Back" button beside the Apps title; present only while an
    /// app group is open.
    pub apps_back: Option<Rect>,
}

/// Compute the layout for the current animation state.
///
/// `surface` is the full surface size, `extent` the card's rise,
/// `n_entries` the total (unfiltered) entry count for the dock,
/// `n_visible` the filtered count per section, `scroll` the unclamped
/// per-section grid offsets.
pub fn layout(
    config: &Config,
    surface: (f32, f32),
    extent: f32,
    n_entries: usize,
    n_visible: [usize; N_SECTIONS],
    scroll: [f32; N_SECTIONS],
    // Which sections are navigated into a sub-view (open app group,
    // entered directory) and get a "‹ Back" pill by their title.
    navigated: [bool; N_SECTIONS],
) -> Layout {
    let (w, h) = surface;
    // The card is centered in the surface with transparent drag margin
    // around it; all card content lays out within these bounds, not
    // the full surface.
    let card_w = w - 2.0 * DRAG_MARGIN_X;
    let card_top = h - extent;
    let dock_h = config.window.input_bar_height as f32;
    let card_h = h - config.window.bottom_margin as f32 - MAGNIFY_HEADROOM - DRAG_MARGIN_TOP;

    // Dock uses n_entries so search never hides dock icons.
    let max_slots = (((card_w - 2.0 * DOCK_PAD_X) / DOCK_SLOT).floor() as usize).max(1);
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
    let inner_w = (card_w - 2.0 * GRID_PAD_X).max(GRID_CELL_W);
    let cols = ((inner_w / GRID_CELL_W).floor() as usize).max(1);

    // Apps takes whatever rows fit after the fixed single-row sections
    // and the three title lines, capped at its design height (6×3 at
    // the default card size); short cards degrade to fewer rows.
    let fixed = (SECTION_ROWS[SECTION_INSTALL] + SECTION_ROWS[SECTION_FILES]) as f32 * GRID_CELL_H
        + N_SECTIONS as f32 * SECTION_TITLE_H
        + (N_SECTIONS - 1) as f32 * SECTION_GAP;
    let avail = grid_bottom - grid_top - fixed;
    let apps_rows = ((avail / GRID_CELL_H) as usize).clamp(1, SECTION_ROWS[SECTION_APPS]);

    let mut y = grid_top;
    let mut files_back = None;
    let mut apps_back = None;
    let sections = std::array::from_fn(|s| {
        let rows = if s == SECTION_APPS {
            apps_rows
        } else {
            SECTION_ROWS[s]
        };
        // An open app group shows a compact, centered 3-wide folder
        // grid (3×3 pages) instead of the full-width strip.
        let cols = if s == SECTION_APPS && navigated[s] {
            GROUP_COLS.min(cols)
        } else {
            cols
        };
        let grid_x0 = (w - cols as f32 * GRID_CELL_W) / 2.0;
        let mut title_pos = (grid_x0 + 8.0, y);
        if navigated[s] {
            // "‹ Back" pill sits where the title starts; title shifts right.
            let back = Rect::new(
                title_pos.0,
                y + (SECTION_TITLE_H - BACK_H) / 2.0 - 1.0,
                BACK_W,
                BACK_H,
            );
            if s == SECTION_FILES {
                files_back = Some(back);
            } else {
                apps_back = Some(back);
            }
            title_pos.0 += BACK_W + 10.0;
        }
        y += SECTION_TITLE_H;
        let viewport = Rect::new(
            grid_x0,
            y,
            cols as f32 * GRID_CELL_W,
            rows as f32 * GRID_CELL_H,
        );
        y += viewport.h + SECTION_GAP;
        // Horizontal paging: each page is viewport.w wide. Pages wrap
        // cyclically (infinite scroll), so the offset is normalized into
        // [0, n_pages * viewport.w) rather than clamped.
        let cells_per_page = cols * rows;
        let n_pages = if n_visible[s] == 0 {
            0
        } else {
            n_visible[s].div_ceil(cells_per_page)
        };
        let total_w = n_pages as f32 * viewport.w;
        let scroll = if total_w > 0.0 {
            scroll[s].rem_euclid(total_w)
        } else {
            0.0
        };
        SectionLayout {
            title_pos,
            viewport,
            cols,
            rows,
            n_pages,
            scroll,
            cells: n_visible[s],
        }
    });

    Layout {
        card_top,
        dock_slots,
        sections,
        search_box,
        search_btn,
        files_back,
        apps_back,
    }
}

/// Return the horizontal scroll offset (page * viewport.w) that makes
/// `cell` visible in its section, snapping to the page containing it.
pub fn scroll_to_reveal(section: &SectionLayout, cell: usize) -> f32 {
    let cells_per_page = (section.cols * section.rows).max(1);
    let page = cell / cells_per_page;
    let max_page = section.n_pages.saturating_sub(1);
    (page.min(max_page) as f32 * section.viewport.w).max(0.0)
}

/// Which section's scroll band contains `pos`: the viewport plus its
/// title line above and the gap (page dots) beneath, so wheel paging is
/// forgiving about the exact pointer height.
pub fn section_at(layout: &Layout, pos: (f32, f32)) -> Option<usize> {
    layout.sections.iter().position(|sec| {
        pos.1 >= sec.viewport.y - SECTION_TITLE_H
            && pos.1 < sec.viewport.y + sec.viewport.h + SECTION_GAP
    })
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
    if !search_open && layout.search_btn.contains(pos) {
        return Some(Hit::SearchButton);
    }
    if layout.files_back.is_some_and(|b| b.contains(pos)) {
        return Some(Hit::FilesBack);
    }
    if layout.apps_back.is_some_and(|b| b.contains(pos)) {
        return Some(Hit::AppsBack);
    }
    for (s, sec) in layout.sections.iter().enumerate() {
        if sec.n_pages > 0 && sec.viewport.contains(pos) {
            // Adjust for horizontal page scroll; pages wrap cyclically.
            let adjusted_x = pos.0 - sec.viewport.x + sec.scroll;
            let page_w = sec.viewport.w.max(1.0);
            let page = (adjusted_x / page_w).floor() as usize % sec.n_pages.max(1);
            let col_f = (adjusted_x - page as f32 * page_w) / GRID_CELL_W;
            let row = ((pos.1 - sec.viewport.y) / GRID_CELL_H).floor() as usize;
            let col = col_f.floor() as usize;
            if col_f >= 0.0 && col < sec.cols && row < sec.rows {
                let cells_per_page = sec.cols * sec.rows;
                let i = page * cells_per_page + row * sec.cols + col;
                if i < sec.cells {
                    return Some(Hit::GridCell(s, i));
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
    /// Section that would accept this drop (install/uninstall target),
    /// washed with a highlight while hovered; `None` otherwise.
    pub drop_section: Option<usize>,
    /// Grid cell that would accept this drop (group create/add),
    /// ringed with a highlight while hovered; `None` otherwise.
    pub over_cell: Option<(usize, usize)>,
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
    /// Magnification amplitude (0..1): the caller fades it around
    /// drags and drops so the wave never pops in or out.
    pub mag_amount: f32,
    /// Launch bounce: (entry index, upward offset in px).
    pub bounce: Option<(usize, f32)>,
    /// Live search query (empty shows the placeholder).
    pub query: &'a str,
    /// Auto-selected grid position (best match while searching), as
    /// (section, cell index within that section's visible list).
    pub selected: Option<(usize, usize)>,
    /// Search box expansion: 0.0 = compact circle button, 1.0 = full pill.
    pub search_expand: f32,
    /// Which entries are using placeholder tiles (no resolved icon file).
    /// Aligned with the `entries` slice passed to `scene()`.
    pub placeholders: &'a [bool],
    /// Texture layer per entry. Normally the entry's own index; dynamic
    /// file-search results borrow a generic asset icon's layer. Empty
    /// slice = identity (entry index is the layer).
    pub layers: &'a [u32],
    /// Directory the Files section is navigated into ("~/…"), shown in
    /// its title; empty at the top level.
    pub files_path: &'a str,
    /// Dock display order: maps slot position → entry index. Empty slice
    /// renders no dock icons (safe default for tests / pre-load frames).
    pub dock_order: &'a [usize],
    /// Active drag for ghost icon and insertion indicator; `None` at rest.
    pub drag: Option<DragFrame>,
    /// Hint shown centered in an empty Install section (index state /
    /// search prompt); empty string falls back to "No results".
    pub install_hint: &'a str,
    /// Entries with a profile mutation in flight, aligned with
    /// `entries`: their cells swap the name for an "Installing…" /
    /// "Removing…" note. Empty slice = nothing busy.
    pub busy: &'a [bool],
    /// Entries whose last mutation failed (flash), aligned with
    /// `entries`: their cells swap the name for "Failed".
    pub failed: &'a [bool],
    /// Group cells: (entry index of the transient group entry, up to
    /// four member texture layers for the 2×2 mini preview). Cells
    /// listed here draw the tile + minis instead of a single icon.
    pub group_minis: &'a [(usize, [Option<u32>; 4])],
    /// Name of the open app group, shown in the Apps title ("Apps —
    /// name"); empty when no group is open.
    pub apps_group: &'a str,
    /// Animated display index per Apps cell (make-room reordering):
    /// cells glide toward these fractional grid positions. Empty =
    /// identity (every cell at its own index).
    pub apps_slide: &'a [f32],
    /// Entry whose grid cell is hidden (the drag's origin — the ghost
    /// is its visual while in flight).
    pub drag_hidden: Option<usize>,
    /// Entry whose dock slot is hidden (a dock-origin drag).
    pub dock_hidden: Option<usize>,
    /// Animated displacement per dock slot, in slot units: the dock's
    /// make-room glide while a drag hovers it. Empty = at rest.
    pub dock_slide: &'a [f32],
    /// Open/close progress of the app-group expand animation (eased,
    /// 0 = collapsed into the tile, 1 = settled). Cells of an open
    /// group scale/glide out of `group_origin`.
    pub group_expand: f32,
    /// Surface point the open group expands from (the clicked tile).
    pub group_origin: Option<(f32, f32)>,
}

/// Assemble the draw scene for one frame.
///
/// `visible` holds, per section, indices into `entries` ranked by the
/// current query — each section's grid shows exactly its list, in
/// order. Dock icons always show the dock order unfiltered.
pub fn scene(
    config: &Config,
    layout: &Layout,
    entries: &[AppEntry],
    visible: &[Vec<usize>; N_SECTIONS],
    surface: (f32, f32),
    frame: &FrameInput,
) -> Scene {
    let FrameInput {
        hover,
        alpha,
        pointer,
        mag_amount,
        bounce,
        query,
        selected,
        search_expand,
        placeholders,
        layers,
        files_path,
        dock_order,
        drag,
        install_hint,
        busy,
        failed,
        group_minis,
        apps_group,
        apps_slide,
        drag_hidden,
        dock_hidden,
        dock_slide,
        group_expand,
        group_origin,
    } = *frame;
    let layer_of = |i: usize| layers.get(i).copied().unwrap_or(i as u32);
    let (w, h) = surface;
    let card_w = w - 2.0 * DRAG_MARGIN_X;
    let card_h = h - config.window.bottom_margin as f32 - MAGNIFY_HEADROOM - DRAG_MARGIN_TOP;
    let mut scene = Scene {
        alpha,
        ..Default::default()
    };
    let lift = |i: usize| match bounce {
        Some((b, offset)) if b == i => offset,
        _ => 0.0,
    };

    // Card background (centered in the surface's drag margin).
    scene.rects.push(RectInst {
        rect: Rect::new(DRAG_MARGIN_X, layout.card_top, card_w, card_h),
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
                    1.0 + (DOCK_MAGNIFY - 1.0)
                        * falloff(px - cx, DOCK_MAG_RADIUS)
                        * fy
                        * mag_amount.clamp(0.0, 1.0)
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
    // Dock icons: slot index → entry index via dock_order. `dock_slide`
    // parts the row around a hovering drag; a dock-origin drag's own
    // icon is hidden (its ghost is in hand).
    for slot in 0..layout.dock_slots.len() {
        let Some(&entry_idx) = dock_order.get(slot) else {
            break;
        };
        if dock_hidden == Some(entry_idx) {
            continue;
        }
        let slot_rect = &layout.dock_slots[slot];
        let vcx = dock_vcx[slot] + dock_slide.get(slot).copied().unwrap_or(0.0) * DOCK_SLOT;
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
            layer: layer_of(entry_idx),
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
    // Drag-and-drop visuals: the ghost (the dock parts via dock_slide
    // instead of drawing an insertion bar).
    if let Some(df) = drag {
        // Wash the section that would accept this drop.
        if let Some(target) = df.drop_section {
            let vp = &layout.sections[target].viewport;
            let hl = config.theme.highlight_rgba();
            scene.rects.push(RectInst {
                rect: Rect::new(vp.x - 6.0, vp.y - 4.0, vp.w + 12.0, vp.h + 8.0),
                radius: 14.0,
                color: [hl[0], hl[1], hl[2], (hl[3] * 1.4).min(0.3)],
            });
        }
        // Ghost following the pointer, topmost: the dragged icon, or a
        // box's mini stack. Slightly enlarged — it's "in hand".
        let size = DOCK_ICON * 1.25;
        let (gx, gy) = df.pos;
        if let Some((_, minis)) = group_minis.iter().find(|(e, _)| *e == df.entry_idx) {
            let m = size * 0.46;
            let gap = size * 0.08;
            for (k, layer) in minis.iter().enumerate() {
                let Some(layer) = layer else { continue };
                let col_k = (k % 2) as f32;
                let row_k = (k / 2) as f32;
                scene.overlay.push(IconInst {
                    rect: Rect::new(
                        gx - m - gap / 2.0 + col_k * (m + gap),
                        gy - m - gap / 2.0 + row_k * (m + gap),
                        m,
                        m,
                    ),
                    layer: *layer,
                });
            }
        } else {
            scene.overlay.push(IconInst {
                rect: Rect::new(gx - size / 2.0, gy - size / 2.0, size, size),
                layer: layer_of(df.entry_idx),
            });
        }
    }

    // Search widget: "Filter" pill button that morphs into an expanding
    // search box. search_expand drives the open animation (0→1).
    {
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
            text_px.max(SEARCH_W_MIN).min(card_w - 2.0 * GRID_PAD_X)
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

    // "‹ Back" pills beside navigated section titles (Files dirs, open
    // app groups).
    for (back, hit) in [
        (layout.files_back, Hit::FilesBack),
        (layout.apps_back, Hit::AppsBack),
    ] {
        let Some(back) = back else {
            continue;
        };
        let hl = config.theme.highlight_rgba();
        let a = if hover == Some(hit) {
            hl[3].min(1.0)
        } else {
            (hl[3] * 0.6).min(1.0)
        };
        scene.rects.push(RectInst {
            rect: back,
            radius: back.h / 2.0,
            color: [hl[0], hl[1], hl[2], a],
        });
        scene.labels.push(Label {
            text: "‹ Back".to_string(),
            pos: (
                back.x + back.w / 2.0,
                back.y + (back.h - LABEL_LINE_PX) / 2.0,
            ),
            max_w: back.w,
            font_px: LABEL_FONT_PX - 1.0,
            line_px: LABEL_LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            clip: None,
        });
    }

    // The three sections: title, grid cells with per-section horizontal
    // paging, page dots, and per-section empty states.
    for (s, sec) in layout.sections.iter().enumerate() {
        let title = if s == SECTION_FILES && !files_path.is_empty() {
            format!("{} — {}", SECTION_TITLES[s], files_path)
        } else if s == SECTION_APPS && !apps_group.is_empty() {
            format!("{} — {}", SECTION_TITLES[s], apps_group)
        } else {
            SECTION_TITLES[s].to_string()
        };
        scene.labels.push(Label {
            text: title,
            pos: (sec.title_pos.0, sec.title_pos.1 + 2.0),
            max_w: sec.viewport.w,
            font_px: LABEL_FONT_PX,
            line_px: LABEL_LINE_PX,
            centered: false,
            dim: true,
            cache: true,
            clip: None,
        });

        let mut grid = GridContent {
            clip: sec.viewport,
            ..Default::default()
        };
        if sec.cells == 0 {
            // Empty state: Install shows its index/search hint;
            // Apps/Files show one only when a search matched nothing.
            let text = match s {
                SECTION_INSTALL if !install_hint.is_empty() => install_hint,
                _ if !query.is_empty() => "No results",
                _ => {
                    scene.grids.push(grid);
                    continue;
                }
            };
            grid.labels.push(Label {
                text: text.to_string(),
                pos: (
                    sec.viewport.x + sec.viewport.w / 2.0,
                    sec.viewport.y + sec.viewport.h / 2.0 - LABEL_LINE_PX / 2.0,
                ),
                max_w: 200.0,
                font_px: LABEL_FONT_PX + 2.0,
                line_px: LABEL_LINE_PX + 2.0,
                centered: true,
                dim: true,
                cache: true,
                clip: None,
            });
            scene.grids.push(grid);
            continue;
        }

        let page_w = sec.viewport.w;
        let cells_per_page = (sec.cols * sec.rows).max(1);
        let total_w = (sec.n_pages as f32 * page_w).max(1.0);
        // A cell's place comes from its display index — fractional
        // while reorder-gliding, so cells ease between grid slots.
        // Pages wrap cyclically; off-screen cells cull.
        let cell_pos = |idx: f32| -> (f32, f32) {
            let i0 = idx.max(0.0).floor() as usize;
            let frac = (idx - i0 as f32).clamp(0.0, 1.0);
            let corner = |i: usize| -> (f32, f32) {
                let page = i / cells_per_page;
                let within = i % cells_per_page;
                let (row, col) = (within / sec.cols, within % sec.cols);
                let rel0 = (page as f32 * page_w - sec.scroll).rem_euclid(total_w);
                let rel = if rel0 >= page_w { rel0 - total_w } else { rel0 };
                (
                    sec.viewport.x + rel + col as f32 * GRID_CELL_W,
                    sec.viewport.y + row as f32 * GRID_CELL_H,
                )
            };
            let (x0, y0) = corner(i0);
            if frac <= f32::EPSILON {
                return (x0, y0);
            }
            let (x1, y1) = corner(i0 + 1);
            (lerp(x0, x1, frac), lerp(y0, y1, frac))
        };
        // Group-open transition progress for this section (1 = settled).
        let expand = if s == SECTION_APPS && group_origin.is_some() {
            group_expand.clamp(0.0, 1.0)
        } else {
            1.0
        };
        for i in 0..sec.cells {
            let Some(&entry_idx) = visible[s].get(i) else {
                break;
            };
            let Some(entry) = entries.get(entry_idx) else {
                break;
            };
            // The drag's origin cell vanishes (its ghost is in hand).
            if drag_hidden == Some(entry_idx) {
                continue;
            }
            let di = if s == SECTION_APPS {
                apps_slide.get(i).copied().unwrap_or(i as f32)
            } else {
                i as f32
            };
            let (mut cell_x, mut cell_y) = cell_pos(di);
            if cell_x - sec.viewport.x <= -GRID_CELL_W || cell_x - sec.viewport.x >= page_w {
                continue;
            }
            // Opening a box: cells scale/glide out of the clicked tile
            // (and back into it while closing).
            if expand < 1.0 {
                if let Some((ox, oy)) = group_origin {
                    cell_x = lerp(ox - GRID_CELL_W / 2.0, cell_x, expand);
                    cell_y = lerp(oy - GRID_CELL_H / 2.0, cell_y, expand);
                }
            }
            let cell = Rect::new(cell_x, cell_y, GRID_CELL_W, GRID_CELL_H);
            if expand >= 1.0 {
                if selected == Some((s, i)) {
                    let hl = config.theme.highlight_rgba();
                    grid.rects.push(RectInst {
                        rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                        radius: 14.0,
                        color: [hl[0], hl[1], hl[2], (hl[3] * 1.8).min(0.4)],
                    });
                } else if hover == Some(Hit::GridCell(s, i)) {
                    grid.rects.push(RectInst {
                        rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                        radius: 14.0,
                        color: config.theme.highlight_rgba(),
                    });
                }
                // A drag hovering a cell that would take the drop
                // (group create/add) rings it brightly.
                if drag.and_then(|d| d.over_cell) == Some((s, i)) {
                    let hl = config.theme.highlight_rgba();
                    grid.rects.push(RectInst {
                        rect: Rect::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0),
                        radius: 16.0,
                        color: [hl[0], hl[1], hl[2], (hl[3] * 2.2).min(0.5)],
                    });
                }
            }
            let cx = cell.x + cell.w / 2.0;
            let icon_cy = cell.y + 12.0 + GRID_ICON / 2.0;
            let grow = lerp(0.35, 1.0, expand);
            if let Some((_, minis)) = group_minis.iter().find(|(e, _)| *e == entry_idx) {
                // Group cell: folder-style tile with a 2×2 mini
                // preview of the first members.
                let hl = config.theme.highlight_rgba();
                let tile = GRID_ICON * 1.15;
                grid.rects.push(RectInst {
                    rect: Rect::new(cx - tile / 2.0, icon_cy - tile / 2.0, tile, tile),
                    radius: 14.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 1.3).min(0.32)],
                });
                let m = GRID_ICON * 0.42;
                let gap = GRID_ICON * 0.10;
                for (k, layer) in minis.iter().enumerate() {
                    let Some(layer) = layer else { continue };
                    let col_k = (k % 2) as f32;
                    let row_k = (k / 2) as f32;
                    grid.icons.push(IconInst {
                        rect: Rect::new(
                            cx - m - gap / 2.0 + col_k * (m + gap),
                            icon_cy - m - gap / 2.0 + row_k * (m + gap),
                            m,
                            m,
                        ),
                        layer: *layer,
                    });
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
                continue;
            }
            let scale = match pointer {
                Some((px, py)) if expand >= 1.0 => {
                    let d = ((px - cx).powi(2) + (py - icon_cy).powi(2)).sqrt();
                    1.0 + (GRID_MAGNIFY - 1.0)
                        * falloff(d, GRID_MAG_RADIUS)
                        * mag_amount.clamp(0.0, 1.0)
                }
                _ => 1.0,
            };
            let size = GRID_ICON * scale * grow;
            grid.icons.push(IconInst {
                rect: Rect::new(
                    cx - size / 2.0,
                    icon_cy - size / 2.0 - lift(entry_idx),
                    size,
                    size,
                ),
                layer: layer_of(entry_idx),
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
            // Names pop in at the end of the expand; a profile
            // mutation in flight swaps the name for a progress note
            // (dimmed); a failed one flashes "Failed".
            if expand < 0.85 {
                continue;
            }
            let is_busy = busy.get(entry_idx).copied().unwrap_or(false);
            let is_failed = failed.get(entry_idx).copied().unwrap_or(false);
            let name = if is_busy && s == SECTION_INSTALL {
                "Installing…".to_string()
            } else if is_busy {
                "Removing…".to_string()
            } else if is_failed {
                "Failed".to_string()
            } else {
                truncate_label(&entry.name, cell.w - 12.0, LABEL_FONT_PX)
            };
            grid.labels.push(Label {
                text: name,
                pos: (cx, cell.y + 12.0 + GRID_ICON + 8.0),
                max_w: cell.w - 12.0,
                font_px: LABEL_FONT_PX,
                line_px: LABEL_LINE_PX,
                centered: true,
                dim: is_busy,
                cache: true,
                clip: None,
            });
        }
        scene.grids.push(grid);

        // Page indicator dots in the gap beneath the section.
        if sec.n_pages > 1 {
            let dot_r = 3.0;
            let dot_spacing = 10.0;
            let total_dots_w = sec.n_pages as f32 * dot_spacing - (dot_spacing - dot_r * 2.0);
            let dot_cx = sec.viewport.x + sec.viewport.w / 2.0;
            let dot_y = sec.viewport.y + sec.viewport.h + SECTION_GAP / 2.0;
            let page_frac = sec.scroll / page_w;
            let hl = config.theme.highlight_rgba();
            for p in 0..sec.n_pages {
                let x = dot_cx - total_dots_w / 2.0 + p as f32 * dot_spacing + dot_r;
                // Cyclic distance: the highlight wraps with the pages.
                let raw = (p as f32 - page_frac).abs();
                let dist = raw.min(sec.n_pages as f32 - raw).min(1.0);
                let alpha = 1.0 - dist * 0.72;
                scene.rects.push(RectInst {
                    rect: Rect::new(x - dot_r, dot_y - dot_r, dot_r * 2.0, dot_r * 2.0),
                    radius: dot_r,
                    color: [hl[0], hl[1], hl[2], hl[3] * alpha],
                });
            }
        }
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
                path: None,
            })
            .collect()
    }

    fn vis(n: usize) -> [Vec<usize>; N_SECTIONS] {
        [(0..n).collect(), Vec::new(), Vec::new()]
    }

    #[test]
    fn unpin_wash_appears_only_when_flagged() {
        let cfg = config();
        let l = open_layout(&cfg, 6, 0.0);
        let make = |unpin| {
            scene(
                &cfg,
                &l,
                &entries(6),
                &vis(6),
                SURFACE,
                &FrameInput {
                    alpha: 1.0,
                    drag: Some(DragFrame {
                        entry_idx: 0,
                        pos: (100.0, 700.0),
                        drop_section: None,
                        over_cell: None,
                        unpin,
                    }),
                    ..Default::default()
                },
            )
            .overlay_rects
            .len()
        };
        assert_eq!(make(false), 0, "no wash without the unpin flag");
        assert_eq!(make(true), 1, "one red wash over the ghost when unpinning");
    }

    fn config() -> Config {
        Config::default()
    }

    /// Surface for the default config: (width + 2·drag margin) ×
    /// (height + bottom margin + headroom + top drag margin).
    const SURFACE: (f32, f32) = (880.0, 772.0);
    /// Fully-open extent for the default config (height + margin);
    /// independent of the drag margins.
    const OPEN: f32 = 692.0;

    fn open_layout(cfg: &Config, n: usize, scroll: f32) -> Layout {
        layout(
            cfg,
            SURFACE,
            OPEN,
            n,
            [n, 0, 0],
            [scroll, 0.0, 0.0],
            [false; N_SECTIONS],
        )
    }

    #[test]
    fn docked_extent_shows_dock_row_only() {
        let cfg = config();
        let l = layout(
            &cfg,
            SURFACE,
            48.0,
            20,
            [20, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
        );
        assert!(!l.dock_slots.is_empty());
        // Sections exist geometrically but lie below the surface
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
        assert!(s.grids.iter().all(|g| g.clip.y >= SURFACE.1));
    }

    #[test]
    fn open_extent_shows_three_sections() {
        let cfg = config();
        let l = open_layout(&cfg, 40, 1e9);
        let apps = &l.sections[SECTION_APPS];
        assert_eq!(apps.cols, 6, "720px card fits exactly 6 columns");
        assert_eq!(apps.rows, 3, "default card height fits the 6×3 grid");
        assert_eq!(l.sections[SECTION_INSTALL].rows, 1);
        assert_eq!(l.sections[SECTION_FILES].rows, 1);
        // Sections stack without overlap and fit above the search box.
        assert!(l.sections[SECTION_INSTALL].viewport.y >= apps.viewport.y + apps.viewport.h);
        let files = &l.sections[SECTION_FILES];
        assert!(files.viewport.y + files.viewport.h <= l.search_box.y);
        // Scroll wraps cyclically into the page strip.
        assert!(apps.scroll >= 0.0 && apps.scroll < apps.n_pages as f32 * apps.viewport.w);
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
        assert_eq!(s.grids.len(), N_SECTIONS);
        let apps_grid = &s.grids[SECTION_APPS];
        assert!(!apps_grid.icons.is_empty());
        assert_eq!(apps_grid.icons.len(), apps_grid.labels.len());
    }

    #[test]
    fn files_section_shows_its_own_entries() {
        let cfg = config();
        let l = layout(
            &cfg,
            SURFACE,
            OPEN,
            10,
            [4, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
        );
        let visible = [vec![0, 1, 2, 3], Vec::new(), vec![4, 5, 6, 7, 8, 9]];
        let s = scene(
            &cfg,
            &l,
            &entries(10),
            &visible,
            SURFACE,
            &FrameInput {
                alpha: 1.0,
                install_hint: "Search to install from nixpkgs",
                ..Default::default()
            },
        );
        assert_eq!(s.grids[SECTION_FILES].icons.len(), 6);
        // Install is empty: no icons, just the hint label.
        assert!(s.grids[SECTION_INSTALL].icons.is_empty());
        assert_eq!(
            s.grids[SECTION_INSTALL].labels[0].text,
            "Search to install from nixpkgs"
        );
    }

    #[test]
    fn negative_scroll_wraps_to_last_page() {
        let cfg = config();
        let page_w = l_page_w(&cfg);
        // One page-width left of page 0 is the last page's position.
        let l = open_layout(&cfg, 72, -page_w);
        let apps = &l.sections[SECTION_APPS];
        assert!(apps.n_pages >= 2, "test needs multiple pages");
        let expect = (apps.n_pages - 1) as f32 * page_w;
        assert!(
            (apps.scroll - expect).abs() < 0.5,
            "scroll={} expected={}",
            apps.scroll,
            expect
        );
    }

    #[test]
    fn hit_test_finds_dock_icon_and_cells_per_section() {
        let cfg = config();
        let l = layout(
            &cfg,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
        );
        let slot = l.dock_slots[0];
        assert_eq!(
            hit_test(&l, (slot.x + 1.0, slot.y + 1.0), false),
            Some(Hit::DockIcon(0))
        );
        // Second cell of the apps section's first row.
        let apps = &l.sections[SECTION_APPS];
        let cell1 = (
            apps.viewport.x + GRID_CELL_W * 1.5,
            apps.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(
            hit_test(&l, cell1, false),
            Some(Hit::GridCell(SECTION_APPS, 1))
        );
        // First cell of the files section.
        let files = &l.sections[SECTION_FILES];
        let fcell = (
            files.viewport.x + GRID_CELL_W / 2.0,
            files.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(
            hit_test(&l, fcell, false),
            Some(Hit::GridCell(SECTION_FILES, 0))
        );
        // The empty install section hits nothing.
        let install = &l.sections[SECTION_INSTALL];
        let icell = (
            install.viewport.x + GRID_CELL_W / 2.0,
            install.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(hit_test(&l, icell, false), None);
        assert_eq!(hit_test(&l, (1.0, 1.0), false), None);
    }

    #[test]
    fn back_button_appears_only_when_navigated() {
        let cfg = config();
        let flat = layout(
            &cfg,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
        );
        assert!(flat.files_back.is_none());
        let nav = layout(
            &cfg,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false, false, true],
        );
        let back = nav.files_back.expect("navigated layout has a back button");
        let center = (back.x + back.w / 2.0, back.y + back.h / 2.0);
        assert_eq!(hit_test(&nav, center, false), Some(Hit::FilesBack));
        // The title moved right to make room.
        assert!(nav.sections[SECTION_FILES].title_pos.0 > flat.sections[SECTION_FILES].title_pos.0);
    }

    #[test]
    fn section_at_routes_scroll_bands() {
        let cfg = config();
        let l = layout(
            &cfg,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
        );
        let apps = &l.sections[SECTION_APPS];
        let mid = (
            apps.viewport.x + 10.0,
            apps.viewport.y + apps.viewport.h / 2.0,
        );
        assert_eq!(section_at(&l, mid), Some(SECTION_APPS));
        let files = &l.sections[SECTION_FILES];
        // The title line above the grid belongs to the section too.
        let title = (files.viewport.x + 10.0, files.viewport.y - 4.0);
        assert_eq!(section_at(&l, title), Some(SECTION_FILES));
        assert_eq!(section_at(&l, (10.0, 0.0)), None);
    }

    #[test]
    fn scrolled_hit_test_offsets_cells() {
        let cfg = config();
        // Scroll exactly one page forward.
        let l = open_layout(&cfg, 18 * 3, l_page_w(&cfg));
        let apps = &l.sections[SECTION_APPS];
        // Top-left of the visible area is now the first cell of page 1.
        let pos = (apps.viewport.x + 2.0, apps.viewport.y + 2.0);
        let cells_per_page = apps.cols * apps.rows;
        assert_eq!(
            hit_test(&l, pos, false),
            Some(Hit::GridCell(SECTION_APPS, cells_per_page))
        );
    }

    fn l_page_w(cfg: &Config) -> f32 {
        open_layout(cfg, 24, 0.0).sections[SECTION_APPS].viewport.w
    }

    #[test]
    fn partial_last_row_is_not_hit_beyond_entries() {
        let cfg = config();
        // 7 entries in >= 2 columns: the last row is partial.
        let l = open_layout(&cfg, 7, 0.0);
        let apps = &l.sections[SECTION_APPS];
        let last_row = 7 / apps.cols;
        let beyond = 7 % apps.cols; // first empty column of the last row
        if beyond > 0 && last_row < apps.rows {
            let pos = (
                apps.viewport.x + (beyond as f32 + 0.5) * GRID_CELL_W,
                apps.viewport.y + (last_row as f32 + 0.5) * GRID_CELL_H,
            );
            assert_eq!(hit_test(&l, pos, false), None);
        }
    }
}

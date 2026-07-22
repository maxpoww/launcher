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
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
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
    /// Glass-material strength: `0.0` → plain solid fill; `1.0` → full
    /// liquid-glass material. Values in between keep all glass layers at
    /// full strength except the iridescent tint, which is scaled by this
    /// factor — so `0.5` gives the same rim glow / spotlight as `1.0` but
    /// half the rainbow shimmer (used for the group-box panel, which is
    /// smaller so edge effects already read more strongly).
    pub glass: f32,
}

/// One icon quad, referencing a texture-array layer.
#[derive(Debug, Clone, Copy)]
pub struct IconInst {
    pub rect: Rect,
    pub layer: u32,
}

/// One continuous soft drop shadow wrapping a rounded rect (the dock). The
/// shader renders only the exterior penumbra (nothing under the shape, so the
/// glass stays clean) and blends the four per-edge strengths by direction, so
/// corners transition smoothly between their two neighbouring edges.
#[derive(Debug, Clone, Copy)]
pub struct ShadowInst {
    /// The rounded rect being shadowed (the dock/card).
    pub rect: Rect,
    /// Corner radius of that rect.
    pub radius: f32,
    /// Penumbra width in px — how far the shadow reaches beyond every edge.
    pub blur: f32,
    /// Straight (non-premultiplied) RGB plus an overall strength in `.a`
    /// (fades with the dock reveal / open). The shader premultiplies.
    pub color: [f32; 4],
    /// Per-edge base darkness: `[top, bottom, left, right]`.
    pub edges: [f32; 4],
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
    /// Edge shadows, drawn first (behind everything).
    pub shadows: Vec<ShadowInst>,
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
    /// Index into `grids` where the open box's overlay begins (the box
    /// panel and its members). Grids before it are the base scene, which
    /// the renderer blurs behind the box; from here on is drawn *over* that
    /// blur. `None` = no box open (nothing to split).
    pub blur_split: Option<usize>,
    /// The open box's rounded rect and corner radius — the region the
    /// renderer fills with a blurred sample of the base scene (frosted
    /// glass). `None` when no box is open.
    pub box_rect: Option<(Rect, f32)>,
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
    /// A member cell of the magnified open box: the 3×3 slot index under
    /// the pointer (0..9). Clicking a filled slot launches it; dragging
    /// pulls the app out of the box. A click on an empty slot is inert —
    /// only a click *outside* the box closes it. Also used for a dock
    /// folder's stack (the same box, anchored above the dock).
    OpenBoxCell(usize),
}

/// Popup sections, top to bottom: the app grid, the (future) install
/// row, and the home-folder row. Each pages independently.
pub const SECTION_APPS: usize = 0;
pub const SECTION_INSTALL: usize = 1;
pub const SECTION_FILES: usize = 2;
pub const N_SECTIONS: usize = 3;
const SECTION_TITLES: [&str; N_SECTIONS] = ["Apps", "Install", "Files"];
/// Grid rows per section (Apps is a cap; it shrinks on short cards).
const SECTION_ROWS: [usize; N_SECTIONS] = [4, 1, 1];
/// Height of a section's title line above its grid.
const SECTION_TITLE_H: f32 = 17.0;
/// Vertical gap beneath each section (page dots live here).
const SECTION_GAP: f32 = 8.0;

/// Fixed layout metrics (logical px). Config-independent for now; can
/// move into `[theme]` if tuning is wanted.
pub const DOCK_ICON: f32 = 40.0;
const DOCK_SLOT: f32 = 44.0;
const DOCK_PAD_X: f32 = 10.0;
pub const GRID_CELL_W: f32 = 104.0;
pub const GRID_CELL_H: f32 = 86.0;
pub const GRID_ICON: f32 = 54.0;
/// Vertical padding inside a grid cell: from the cell top to the icon,
/// and from the icon bottom to its label. Kept tight so more rows fit.
pub const GRID_ICON_TOP: f32 = 9.0;
const GRID_LABEL_GAP: f32 = 5.0;
/// A closed box tile's side (the folder-style square that holds the 2×2
/// mini preview) — the size the magnified box grows out of / collapses to.
pub const BOX_TILE: f32 = GRID_ICON * 1.15;
/// Columns/rows of the magnified open box's inner app grid (3×3 = up to
/// nine members shown per page).
pub const OPEN_BOX_COLS: usize = 3;
/// Members per open-box page — the box grid's capacity, and the page
/// size of a group's member list (`groups::PAGE_CAP` re-exports it).
pub const OPEN_BOX_CAP: usize = OPEN_BOX_COLS * OPEN_BOX_COLS;
const GRID_PAD_X: f32 = 14.0;
const GRID_TOP_GAP: f32 = 5.0;
const GRID_BOTTOM_PAD: f32 = 6.0;
/// Text metrics for the app-name labels.
pub const LABEL_FONT_PX: f32 = 12.0;
pub const LABEL_LINE_PX: f32 = 16.0;
/// Search box: height, inner padding, text metrics.
const SEARCH_H: f32 = 28.0;
/// Width of the collapsed "Filter" button pill.
const SEARCH_BTN_W: f32 = 80.0;
/// Minimum expanded width — the resting size; the pill grows from here
/// with the shaped query.
const SEARCH_W_MIN: f32 = 160.0;
const SEARCH_PAD_X: f32 = 14.0;
const SEARCH_GAP: f32 = 7.0;
pub(crate) const SEARCH_FONT_PX: f32 = 15.0;
const SEARCH_LINE_PX: f32 = 20.0;

/// How much the content ride trails the opening card (1.0 = glued to
/// it): the parallax depth of the rise-from-the-bottom-edge animation.
const RIDE_PARALLAX: f32 = 1.18;

/// AGUA squash & stretch: the card's speed drives a small underdamped
/// stretch spring anchored at the pinned bottom — content elongates
/// while rising, keeps sloshing briefly after the card lands, and
/// relaxes to exactly 1.0. Deformation per px/s of card speed, its
/// caps, and the per-body followers. Each water body chases the same
/// target at its own tempo — the icons a touch ahead and bouncier, the
/// content a breath behind and calmer, the card silhouette in between
/// (overlapping action: their swells peak at slightly different
/// moments, like real water).
pub(crate) const STRETCH_PER_VEL: f32 = 0.012;
pub(crate) const STRETCH_MAX: f32 = 0.10;
pub(crate) const SQUASH_MAX: f32 = 0.05;
/// Card silhouette: ζ≈0.5 — swells and rebounds once or twice.
pub(crate) const AGUA_CARD_K: f32 = 120.0;
pub(crate) const AGUA_CARD_C: f32 = 11.0;
/// Softer spring for the jelly edge expansion — same impulse as agua_card
/// but lower stiffness/damping so it rings longer after the box opens.
pub(crate) const AGUA_BREATH_K: f32 = 40.0;
pub(crate) const AGUA_BREATH_C: f32 = 4.5;
/// Snappy spring for the pointer edge-poke: stiff + lightly damped so the
/// edge shoots back and bounces once or twice before resting.
pub(crate) const JELLY_K: f32 = 160.0;
pub(crate) const JELLY_C: f32 = 6.0; // lower damping = more ring = more alive
/// Velocity impulse (pos-units/s) injected on each edge crossing.
pub(crate) const JELLY_KICK: f32 = 11.0;
/// Scale from pos-deviation (around 1.0) to pixels of edge displacement.
pub(crate) const JELLY_SCALE: f32 = 2.0;

/// Corner radius shared by every "box" — the fully-open main card and the
/// pop-out folder boxes all round to this. Clearly rounder than the docked
/// card (`theme.corner_radius`), which stays subtle for a macOS-dock look.
pub(crate) const BOX_CORNER_RADIUS: f32 = 24.0;

/// Dock drop shadow. One continuous soft shadow wraps the whole rounded
/// dock; `BLUR` is how far the penumbra reaches out from every edge, and the
/// four `ALPHA`s set per-edge darkness (corners blend the two neighbours, so
/// there are no seams). Alphas stay under the compositor's `ignore_alpha`
/// blur threshold so the shadow never triggers layer blur.
pub(crate) const DOCK_SHADOW_BLUR: f32 = 16.0;
pub(crate) const DOCK_SHADOW_ALPHA_TOP: f32 = 0.16;
pub(crate) const DOCK_SHADOW_ALPHA_BOTTOM: f32 = 0.62;
pub(crate) const DOCK_SHADOW_ALPHA_SIDE: f32 = 0.16;
/// Dock icons: fastest and bounciest (ζ≈0.3), first to peak, a few
/// visible sloshes.
pub(crate) const AGUA_ICONS_K: f32 = 200.0;
pub(crate) const AGUA_ICONS_C: f32 = 8.5;
/// Box content: slowest (ζ≈0.45), clearly trailing, sloshing softly.
pub(crate) const AGUA_CONTENT_K: f32 = 65.0;
pub(crate) const AGUA_CONTENT_C: f32 = 7.3;
/// Dock icons are small (a few dozen px), so their deformation gets a
/// gain over the raw factor or it reads as nothing.
const DOCK_STRETCH_GAIN: f32 = 1.6;
/// The box content spans hundreds of px, so it takes the raw factor
/// *down* — tuned live: the dock landed at 1.6x; the content stepped
/// 1x -> 0.5x -> 0.3x -> 0.22x.
const CONTENT_STRETCH_GAIN: f32 = 0.22;

/// Gap between an open dock-folder box and the dock icon it springs from.
const DOCK_BOX_GAP: f32 = 12.0;

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
/// AGUA silhouette: the docked basin keeps only this side margin, so
/// the resting bar spreads wider than the risen box.
const BASIN_MARGIN_X: f32 = 16.0;
/// Volume conservation: how much of the vertical stretch converts to
/// horizontal narrowing (and the landing squash to a widening spill).
const SPILL: f32 = 0.6;
pub const DRAG_MARGIN_TOP: f32 = 56.0;
/// Peak scale of a dock icon directly under the cursor.
pub(crate) const DOCK_MAGNIFY: f32 = 1.5;
/// Horizontal falloff radius of dock magnification, in pixels.
pub(crate) const DOCK_MAG_RADIUS: f32 = 120.0;
/// Vertical attenuation radius, measured from the dock band's edges
/// (zero inside the band). Small on purpose: the effect must die out
/// before the first grid row so hovering the grid never stirs the dock.
pub(crate) const DOCK_MAG_VRADIUS: f32 = 28.0;
/// AGUA splash ripple (decoration only — hover magnification is
/// untouched): when a crest collapses (the pointer leaves or jumps),
/// the falling swell pushes the surface down and a small wave expands
/// across the neighbors, reflecting off the dock ends and decaying.
/// Neighbor coupling (ripple travel speed).
pub(crate) const RIPPLE_COUPLE: f32 = 700.0;
/// Pull of the surface back to flat.
pub(crate) const RIPPLE_RETURN: f32 = 120.0;
/// Damping: lower = the ripple travels farther and lives longer.
pub(crate) const RIPPLE_DAMP: f32 = 5.5;
/// How much of a collapsing crest's fall converts into splash velocity.
pub(crate) const SPLASH_GAIN: f32 = 3.0;
/// Cap on the ripple's contribution to an icon's scale (±).
pub(crate) const RIPPLE_MAX: f32 = 0.04;
/// Running-indicator dot radius (px) and its gap below the icon baseline.
const DOCK_DOT_R: f32 = 2.0;
const DOCK_DOT_GAP: f32 = 3.0;
/// Width (px) of the divider between the pinned and running-unpinned zones.
const DOCK_DIVIDER_W: f32 = 1.5;
/// Peak scale of a grid icon under the cursor.
const GRID_MAGNIFY: f32 = 1.22;
/// Radial falloff radius of grid magnification, in pixels.
const GRID_MAG_RADIUS: f32 = 95.0;

/// Cosine ease from 1.0 at `d == 0` to 0.0 at `d >= radius`.
pub(crate) fn falloff(d: f32, radius: f32) -> f32 {
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
    /// Static lead cells pinned at the viewport's start (the navigated
    /// Files listing's ".." tile): they ignore the scroll, and the
    /// remaining cells page in the strip beside them.
    pub lead: usize,
}

/// Geometry shared by scene assembly and hit-testing.
#[derive(Debug)]
pub struct Layout {
    /// Card left edge / width for the current extent: the AGUA
    /// silhouette — wide at rest in the dock basin, gathered to the box
    /// width as it rises, spilling wider on the landing squash.
    pub card_x: f32,
    pub card_w: f32,
    /// Card top edge in surface coordinates for the current extent.
    pub card_top: f32,
    /// Card height for the current extent: its bottom edge is pinned a
    /// small gap above the screen edge (which the docked bar lifts to as
    /// it reveals), so only the top rises as the box grows.
    pub card_h: f32,
    /// How far down each dock icon's column stays hoverable/clickable:
    /// the screen edge while docked (so the floating gap and the very
    /// bottom edge react), the dock-band bottom once open (the grid owns
    /// everything below it).
    pub dock_hit_bottom: f32,
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
    /// The rectangle the magnified box fills — the Apps grid for a grid box,
    /// or a square above the dock for a dock folder's stack. Present only
    /// while a box is open. A click inside is inert (only outside closes).
    pub open_box: Option<Rect>,
}

/// Compute the layout for the current animation state.
///
/// `surface` is the full surface size, `extent` the card's rise,
/// `n_entries` the total (unfiltered) entry count for the dock,
/// `n_visible` the filtered count per section, `scroll` the unclamped
/// per-section grid offsets.
// Geometry naturally takes many independent inputs; a params struct
// would just rename them.
/// The four icon-size levels, indexed by `App::icon_size` (default 1).
pub const ICON_SCALES: [f32; 4] = [0.70, 1.00, 1.30, 1.65];

#[allow(clippy::too_many_arguments)]
pub fn layout(
    config: &Config,
    icon_scale: f32,
    surface: (f32, f32),
    extent: f32,
    n_entries: usize,
    n_visible: [usize; N_SECTIONS],
    scroll: [f32; N_SECTIONS],
    // Which sections are navigated into a sub-view (open app group,
    // entered directory).
    navigated: [bool; N_SECTIONS],
    // AGUA deformation, (card silhouette, content ride) — 1.0 = rest.
    stretch: (f32, f32),
) -> Layout {
    // Scaled size metrics — all icon/slot dimensions respond to icon_scale.
    let dock_slot    = DOCK_SLOT    * icon_scale;
    let dock_pad_x   = DOCK_PAD_X   * icon_scale;
    let grid_cell_w  = GRID_CELL_W  * icon_scale;
    let grid_cell_h  = GRID_CELL_H  * icon_scale;

    let (w, h) = surface;
    // The card is centered in the surface with transparent drag margin
    // around it; all card content lays out within these bounds, not
    // the full surface.
    let card_w = w - 2.0 * DRAG_MARGIN_X;
    let card_top = h - extent;
    let dock_h    = config.window.input_bar_height as f32 * icon_scale;
    let float_gap = config.window.bottom_margin     as f32 * icon_scale;
    // Hidden↔Dock: the bar keeps its full dock height and just slides up
    // out of the screen edge (clipped by it), lifting to float over the
    // last `float_gap` of travel — its shape never changes. Dock↔Open:
    // it grows upward from the pinned floating bottom.
    let card_h = (extent - float_gap).max(dock_h);
    // AGUA silhouette: at rest the water spreads wide in its basin;
    // rising gathers it to the box width; the live stretch conserves
    // volume — taller is narrower, and the landing squash spills wider.
    let full_extent = (config.window.height + config.window.bottom_margin) as f32 * icon_scale;
    let dock_extent = dock_h + float_gap;
    let rise = ((extent - dock_extent) / (full_extent - dock_extent).max(1.0)).clamp(0.0, 1.0);
    let basin_w = w - 2.0 * BASIN_MARGIN_X;
    let gathered = basin_w + (card_w - basin_w) * rise;
    let card_w_now = (gathered * (1.0 - (stretch.0 - 1.0) * SPILL)).min(w);
    let card_x = (w - card_w_now) / 2.0;
    // Content is SHAPED at its fully-open geometry (`content_top` is
    // where the grid begins when open; `content_bottom` the pinned card
    // bottom) so rows never reflow mid-animation — then translated by
    // `y_off` so it rides the card: rising from the bottom edge as it
    // opens, sinking back into it as it closes (the reveal band clips
    // whatever sits past the card interior).
    let content_top = h - (config.window.height + config.window.bottom_margin) as f32 * icon_scale;
    let content_bottom = h - float_gap;
    // Parallax: the content trails the card a touch (it sits deeper in
    // the stack) and catches up exactly as the card lands — the offset
    // converges to zero at full extent either way.
    let y_off = (card_top - content_top).max(0.0) * RIDE_PARALLAX;
    // AGUA: squash & stretch about the pinned card bottom — content
    // elongates upward while the card moves fast and relaxes to rest
    // as the spring settles (see frame::draw for the velocity link).
    // The content takes the factor at reduced gain (its span is large).
    let cs = 1.0 + (stretch.1 - 1.0) * CONTENT_STRETCH_GAIN;
    let agua = |y: f32| content_bottom - (content_bottom - y) * cs;
    // Docked (a pure bar), the icon columns stay live all the way to the
    // screen edge so the floating gap is clickable; open, they stop at
    // the dock-band bottom and the grid takes over below.
    let dock_hit_bottom = if card_h > dock_h {
        card_top + dock_h
    } else {
        h
    };

    // Dock uses n_entries so search never hides dock icons.
    let max_slots = (((card_w - 2.0 * dock_pad_x) / dock_slot).floor() as usize).max(1);
    let n_dock = n_entries.min(max_slots);
    let start_x = (w - n_dock as f32 * dock_slot) / 2.0;
    let dock_slots = (0..n_dock)
        .map(|i| {
            Rect::new(
                start_x + i as f32 * dock_slot,
                card_top + (dock_h - dock_slot).max(0.0) / 2.0,
                dock_slot,
                dock_slot.min(dock_h),
            )
        })
        .collect();

    // Compact button and minimum-width search box; scene() stretches the
    // box dynamically with the query so layout just anchors the y position.
    let search_box = Rect::new(
        (w - SEARCH_W_MIN) / 2.0,
        agua(content_bottom - GRID_BOTTOM_PAD - SEARCH_H + y_off),
        SEARCH_W_MIN,
        SEARCH_H,
    );
    let search_btn = Rect::new(
        (w - SEARCH_BTN_W) / 2.0,
        search_box.y,
        SEARCH_BTN_W,
        SEARCH_H,
    );

    let grid_top = content_top + dock_h + GRID_TOP_GAP;
    let grid_bottom = (search_box.y - SEARCH_GAP).min(h);
    let inner_w = (card_w - 2.0 * GRID_PAD_X).max(grid_cell_w);
    let cols = ((inner_w / grid_cell_w).floor() as usize).max(1);

    // Apps takes whatever rows fit after the fixed single-row sections
    // and the three title lines, capped at its design height (6×3 at
    // the default card size); short cards degrade to fewer rows.
    let fixed = (SECTION_ROWS[SECTION_INSTALL] + SECTION_ROWS[SECTION_FILES]) as f32 * grid_cell_h
        + N_SECTIONS as f32 * SECTION_TITLE_H
        + (N_SECTIONS - 1) as f32 * SECTION_GAP;
    let avail = grid_bottom - grid_top - fixed;
    let apps_rows = ((avail / grid_cell_h) as usize).clamp(1, SECTION_ROWS[SECTION_APPS]);

    let mut y = grid_top;
    let sections = std::array::from_fn(|s| {
        let rows = if s == SECTION_APPS {
            apps_rows
        } else {
            SECTION_ROWS[s]
        };
        // Apps stays full-width even with a box open (the magnified box
        // fills the whole grid, drawn as an overlay in `scene`).
        let grid_x0 = (w - cols as f32 * grid_cell_w) / 2.0;
        let title_pos = (grid_x0 + 8.0, agua(y + y_off));
        y += SECTION_TITLE_H;
        let viewport = Rect::new(
            grid_x0,
            agua(y + y_off),
            cols as f32 * grid_cell_w,
            rows as f32 * grid_cell_h,
        );
        y += viewport.h + SECTION_GAP;
        // Horizontal paging: each page is viewport.w wide. Pages wrap
        // cyclically (infinite scroll), so the offset is normalized into
        // [0, n_pages * viewport.w) rather than clamped.
        let cells_per_page = cols * rows;
        // A navigated Files listing pins its ".." as a static lead cell;
        // only the strip beside it pages.
        let lead = usize::from(navigated[s] && s == SECTION_FILES);
        let n_pages = if n_visible[s] == 0 {
            0
        } else {
            let strip_cpp = cells_per_page.saturating_sub(lead).max(1);
            n_visible[s].saturating_sub(lead).div_ceil(strip_cpp).max(1)
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
            lead,
        }
    });

    // A box open shows a square as tall as the whole Apps grid (top to
    // bottom edges), centered horizontally — its rest rect, which the
    // magnified box grows into and which swallows clicks.
    let open_box = navigated[SECTION_APPS].then(|| {
        let vp = sections[SECTION_APPS].viewport;
        // Size the box at exactly 3 scaled cells (OPEN_BOX_COLS rows/cols)
        // so icons inside track icon_scale directly. Cap at the viewport.
        let side = (OPEN_BOX_COLS as f32 * grid_cell_h).min(vp.h).min(vp.w);
        Rect::new(vp.x + (vp.w - side) / 2.0, vp.y, side, side)
    });

    Layout {
        card_x,
        card_w: card_w_now,
        card_top,
        card_h,
        dock_hit_bottom,
        dock_slots,
        sections,
        search_box,
        search_btn,
        open_box,
    }
}

/// Return the horizontal scroll offset (page * viewport.w) that makes
/// `cell` visible in its section, snapping to the page containing it.
pub fn scroll_to_reveal(section: &SectionLayout, cell: usize) -> f32 {
    let cells_per_page = (section.cols * section.rows).max(1);
    // Lead cells are static (page 0); strip cells page past the lead.
    let page = if cell < section.lead {
        0
    } else {
        (cell - section.lead) / cells_per_page.saturating_sub(section.lead).max(1)
    };
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

/// The resting square of a dock folder's open box: a `side`-sized square
/// centered over `dock_icon` and floating `DOCK_BOX_GAP` above it, clamped
/// within `surface_w`.
pub fn dock_box_rect(dock_icon: Rect, side: f32, surface_w: f32) -> Rect {
    let cx = dock_icon.x + dock_icon.w / 2.0;
    let x = (cx - side / 2.0).clamp(
        DRAG_MARGIN_X,
        (surface_w - DRAG_MARGIN_X - side).max(DRAG_MARGIN_X),
    );
    Rect::new(x, dock_icon.y - DOCK_BOX_GAP - side, side, side)
}

/// Which item (if any) the pointer is over.
///
/// `search_open` controls whether the compact search button is a target
/// (it is only when the search box is collapsed).
/// What lives at `pos`. `apps_slots` maps the Apps section's display
/// slots (its geometry — pages may have empty tail cells) back to item
/// indices, so a `Hit::GridCell(section, i)` always carries a *list
/// index* into `visible[section]`, never a raw slot: an empty Apps slot
/// is no hit at all. Other sections are dense (slot == index).
pub fn hit_test(
    layout: &Layout,
    pos: (f32, f32),
    search_open: bool,
    apps_slots: &[usize],
) -> Option<Hit> {
    // Each icon's column is live from the top of the dock band down to
    // `dock_hit_bottom` — so a click/hover in the floating gap under an
    // icon (down to the very screen edge) still lands on it.
    let (px, py) = pos;
    if py >= layout.card_top && py < layout.dock_hit_bottom {
        for (i, slot) in layout.dock_slots.iter().enumerate() {
            if px >= slot.x && px < slot.x + slot.w {
                return Some(Hit::DockIcon(i));
            }
        }
    }
    if !search_open && layout.search_btn.contains(pos) {
        return Some(Hit::SearchButton);
    }
    // The magnified open box: resolve the 3×3 member slot under the
    // pointer (clicks inside are inert/launch; only outside closes).
    if let Some(b) = layout.open_box {
        if b.contains(pos) {
            let cols = OPEN_BOX_COLS as f32;
            let col = (((px - b.x) / (b.w / cols)).floor() as usize).min(OPEN_BOX_COLS - 1);
            let row = (((py - b.y) / (b.h / cols)).floor() as usize).min(OPEN_BOX_COLS - 1);
            return Some(Hit::OpenBoxCell(row * OPEN_BOX_COLS + col));
        }
    }
    for (s, sec) in layout.sections.iter().enumerate() {
        if sec.n_pages > 0 && sec.viewport.contains(pos) {
            // Adjust for horizontal page scroll; pages wrap cyclically.
            let adjusted_x = pos.0 - sec.viewport.x + sec.scroll;
            let page_w = sec.viewport.w.max(1.0);
            let page = (adjusted_x / page_w).floor() as usize % sec.n_pages.max(1);
            // Derive scaled cell dimensions from the already-scaled layout geometry.
            let cell_w = sec.viewport.w / sec.cols.max(1) as f32;
            let cell_h = sec.viewport.h / sec.rows.max(1) as f32;
            let col_f = (adjusted_x - page as f32 * page_w) / cell_w;
            let row = ((pos.1 - sec.viewport.y) / cell_h).floor() as usize;
            let col = col_f.floor() as usize;
            // Static lead cells claim the viewport's first columns on
            // every page (the strip slides beneath them).
            if sec.lead > 0 {
                let screen_col = ((pos.0 - sec.viewport.x) / cell_w).floor() as usize;
                if pos.0 >= sec.viewport.x && screen_col < sec.lead {
                    return (screen_col < sec.cells).then_some(Hit::GridCell(s, screen_col));
                }
            }
            if col_f >= 0.0 && col < sec.cols && row < sec.rows {
                let cells_per_page = sec.cols * sec.rows;
                let i = if sec.lead > 0 {
                    if col < sec.lead {
                        return None; // page-space under the lead: nothing
                    }
                    let strip_cpp = cells_per_page.saturating_sub(sec.lead).max(1);
                    sec.lead + page * strip_cpp + (row * sec.cols + col - sec.lead)
                } else {
                    page * cells_per_page + row * sec.cols + col
                };
                if i < sec.cells {
                    // Apps cells are display slots: resolve to the item
                    // occupying the slot (an empty tail cell is no hit).
                    if s == SECTION_APPS {
                        return apps_slots
                            .iter()
                            .position(|&slot| slot == i)
                            .map(|j| Hit::GridCell(s, j));
                    }
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
    /// Dock slot that would accept this drop as a fold (join/create a box),
    /// ringed while hovered; `None` otherwise.
    pub over_dock: Option<usize>,
}

/// Per-frame dynamic inputs to scene assembly.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInput<'a> {
    /// Item under the pointer (hover highlight).
    pub hover: Option<Hit>,
    /// Shaped pixel width of the live query (the caret anchor).
    pub query_px: f32,
    /// AGUA stretch factor (1.0 at rest): dock icons slosh vertically
    /// about their baseline with the card's motion — on the dock
    /// reveal, and at the top of the card as an open lands.
    pub stretch: f32,
    /// Dock slot whose name tooltip should show — gated behind a hover
    /// dwell (the highlight is immediate; the tooltip waits). `None`
    /// while the dwell hasn't elapsed or the pointer is elsewhere.
    pub dock_tooltip: Option<usize>,
    /// Card opacity from the animation.
    pub alpha: f32,
    /// Pointer position driving the macOS-style magnification (icons
    /// swell under the cursor with cosine falloff; purely visual,
    /// hit-boxes stay fixed).
    pub pointer: Option<(f32, f32)>,
    /// Magnification amplitude (0..1): the caller fades it around
    /// drags and drops so the wave never pops in or out.
    pub mag_amount: f32,
    /// AGUA splash ripple per dock slot (0 = flat): a decoration added
    /// on top of the untouched hover magnification. Fed only when a
    /// crest collapses, so hovering never dilutes the magnification.
    /// Empty = no ripple.
    pub dock_ripple: &'a [f32],
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
    /// Per-slot running flag, aligned with `dock_order`: `true` draws the
    /// macOS-style running indicator dot under that icon. Short/empty
    /// slice ⇒ no dots.
    pub dock_running: &'a [bool],
    /// Dock slot at which the running-but-unpinned zone begins: a thin
    /// divider is drawn just left of it. `None` (or 0) draws no divider.
    pub dock_divider: Option<usize>,
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
    /// Busy entries that are *installing* rather than removing, aligned
    /// with `entries`: their cells say "Installing…" even outside the
    /// Install section (drag-to-install tiles live in the Apps grid).
    pub installing: &'a [bool],
    /// Entries being realized for an ephemeral "try it" run, aligned with
    /// `entries`: their cells say "Launching…".
    pub launching: &'a [bool],
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
    /// 0 = collapsed into the tile, 1 = settled). The magnified box
    /// grows from `group_origin` to fill the Apps grid.
    pub group_expand: f32,
    /// Surface point the open box grows from (the clicked tile).
    pub group_origin: Option<(f32, f32)>,
    /// While a box is open: entry indices of its members for the
    /// magnified 3×3 app grid. Empty when no box is open.
    pub open_box_members: &'a [usize],
    /// Animated display slot of each member (`page * OPEN_BOX_CAP +
    /// within`, parallel to `open_box_members`): box pages may be
    /// under-full, so slots — not list positions — decide where a member
    /// draws. Fractional mid-glide (the box's make-room reflow, computed
    /// in `draw` like the grid's `apps_slide`).
    pub open_box_disp: &'a [f32],
    /// The open box's own grid cell (its tile), hidden from the grid
    /// behind while the magnified box stands in for it.
    pub open_box_hidden: Option<usize>,
    /// Open box paging: (current page, total pages). Page dots show when
    /// there is more than one.
    pub open_box_pages: (usize, usize),
    /// Horizontal page-scroll offset of the open box (px): members are laid
    /// out in a strip of 3×3 pages shifted by this, so pages slide.
    pub box_scroll: f32,
    /// In-box member reorder: (dragged member entry index, gap display
    /// slot, pointer pos). The dragged member is hidden and drawn as a
    /// ghost; its page compacts and the gap's page parts around the gap.
    pub box_drag: Option<(usize, usize, (f32, f32))>,
    /// Vertical offset (px) of the card background from the idle breathing
    /// oscillation. Positive = card rises; icons are NOT shifted — the glass
    /// surface floats while content stays anchored to the screen.
    pub breath_offset: f32,
    /// Per-edge poke offsets (px): (left, right, top, bottom).
    /// Each spring moves only its own edge; icons stay anchored.
    /// Positive = edge moves inward (left→right, top→down, right→left, bottom→up).
    pub card_push: (f32, f32, f32, f32),
    /// Same per-edge jelly offsets for the open group box panel.
    pub box_push: (f32, f32, f32, f32),
    /// Dock→open progress (0 = docked, 1 = fully open). Rounds the card
    /// corners from `theme.corner_radius` (dock) to `BOX_CORNER_RADIUS` (open).
    pub card_open: f32,
    /// Card drop-shadow strength: `1.0` while revealed (docked or open as the
    /// main box), fading to `0.0` when hidden. Emits the card's wraparound
    /// shadow in both the docked and open states.
    pub card_shadow: f32,
    /// Whether the open box is floating on top of the dock (a dock/dir
    /// stack) rather than being a grid box inside the launcher. Gates the
    /// box's drop shadow — only floating-on-dock boxes get one.
    pub box_over_dock: bool,
}

/// Local member index shown in 3×3 slot `slot` (row-major) of an open box
/// on the `left` (or right) side of the grid — the inverse of the
/// member→slot placement used when drawing. Lets a click on a slot resolve
/// to the member under it.
pub fn open_box_slot_member(left: bool, slot: usize) -> usize {
    let slot_member: [usize; 9] = if left {
        [0, 1, 3, 2, 4, 5, 6, 7, 8]
    } else {
        [2, 0, 1, 4, 5, 3, 6, 7, 8]
    };
    slot_member.get(slot).copied().unwrap_or(slot)
}

/// Assemble the draw scene for one frame.
///
/// `visible` holds, per section, indices into `entries` ranked by the
/// current query — each section's grid shows exactly its list, in
/// order. Dock icons always show the dock order unfiltered.
pub fn scene(
    config: &Config,
    icon_scale: f32,
    layout: &Layout,
    entries: &[AppEntry],
    visible: &[Vec<usize>; N_SECTIONS],
    surface: (f32, f32),
    frame: &FrameInput,
) -> Scene {
    let FrameInput {
        hover,
        query_px,
        stretch,
        dock_tooltip,
        alpha,
        pointer,
        mag_amount,
        dock_ripple,
        bounce,
        query,
        selected,
        search_expand,
        placeholders,
        layers,
        files_path,
        dock_order,
        dock_running,
        dock_divider,
        drag,
        install_hint,
        busy,
        failed,
        installing,
        launching,
        group_minis,
        apps_group,
        apps_slide,
        drag_hidden,
        dock_hidden,
        dock_slide,
        group_expand,
        group_origin,
        open_box_members,
        open_box_disp,
        open_box_hidden,
        open_box_pages,
        box_scroll,
        box_drag,
        breath_offset,
        card_push,
        box_push,
        card_open,
        card_shadow,
        box_over_dock,
    } = *frame;
    let layer_of = |i: usize| layers.get(i).copied().unwrap_or(i as u32);
    // Scaled drawing metrics — mirrors what layout() received.
    let dock_icon    = DOCK_ICON   * icon_scale;
    let dock_slot    = DOCK_SLOT   * icon_scale;
    let dock_magnify = 1.0 + (DOCK_MAGNIFY - 1.0) * icon_scale;
    let grid_icon    = GRID_ICON   * icon_scale;
    let grid_icon_top = GRID_ICON_TOP * icon_scale;
    let box_tile     = BOX_TILE    * icon_scale;
    let grid_cell_w  = GRID_CELL_W * icon_scale;
    let grid_cell_h  = GRID_CELL_H * icon_scale;
    // The magnified open box's current rect (grows from the tile into its
    // rest square). Used to draw the box and to hide the grid labels it
    // covers — otherwise those names paint over it in the later text pass.
    let open_box_rect = match (open_box_members.is_empty(), group_origin, layout.open_box) {
        (false, Some((ox, oy)), Some(to)) => {
            let t = group_expand.clamp(0.0, 1.0);
            let from = Rect::new(ox - box_tile / 2.0, oy - box_tile / 2.0, box_tile, box_tile);
            Some(Rect::new(
                lerp(from.x, to.x, t),
                lerp(from.y, to.y, t),
                lerp(from.w, to.w, t),
                lerp(from.h, to.h, t),
            ))
        }
        _ => None,
    };
    let (w, _h) = surface;
    let card_w = w - 2.0 * DRAG_MARGIN_X;
    let card_h = layout.card_h;
    // The box content sits at fixed positions; the opening card uncovers
    // it from the bottom up. Everything below the dock band is clipped to
    // this reveal region, whose top (the dock's underside) rises as the
    // box grows — so rows are exposed in place, never moved.
    let dock_h = config.window.input_bar_height as f32 * icon_scale;
    let reveal_top = layout.card_top + dock_h;
    let reveal_bottom = layout.card_top + card_h;
    let reveal_rect = Rect::new(
        layout.card_x,
        reveal_top,
        layout.card_w,
        (reveal_bottom - reveal_top).max(0.0),
    );
    let reveal_clip = |r: Rect| -> Rect {
        let top = r.y.max(reveal_top);
        let bottom = (r.y + r.h).min(reveal_bottom);
        Rect::new(r.x, top, r.w, (bottom - top).max(0.0))
    };
    let mut scene = Scene {
        alpha,
        ..Default::default()
    };
    let lift = |i: usize| match bounce {
        Some((b, offset)) if b == i => offset,
        _ => 0.0,
    };

    // Card background: the AGUA silhouette — wide basin at rest,
    // gathered to the box width risen, spilling on the landing.
    // `breath_offset` expands all four edges equally (card center fixed,
    // icons stay anchored); positive = exhale (card grows), negative = inhale.
    // Per-edge jelly: each spring moves only its own edge.
    // (left, right, top, bottom) offsets; positive = inward.
    // Combined with breath (uniform expansion from all edges).
    let (jl, jr, jt, jb) = card_push;
    let card_rect = Rect::new(
        layout.card_x   - breath_offset + jl,
        layout.card_top - breath_offset + jt,
        layout.card_w + 2.0 * breath_offset - jl + jr,
        card_h + 2.0 * breath_offset - jt + jb,
    );
    // Subtle/macOS-dock rounding while docked, rounding up to the shared box
    // radius as the card opens — so the open card matches the folder boxes.
    let card_radius = lerp(config.theme.corner_radius, BOX_CORNER_RADIUS, card_open);
    // One continuous soft drop shadow around the whole rounded card — the
    // dock when docked and the main box when open (fades out only when
    // hidden). Per-edge strengths keep the top and sides subtle and the
    // bottom strong; corners blend their neighbours so there are no seams.
    if card_shadow > 0.001 {
        scene.shadows.push(ShadowInst {
            rect: card_rect,
            radius: card_radius,
            blur: DOCK_SHADOW_BLUR,
            color: [0.0, 0.0, 0.0, card_shadow.clamp(0.0, 1.0)],
            edges: [
                DOCK_SHADOW_ALPHA_TOP,
                DOCK_SHADOW_ALPHA_BOTTOM,
                DOCK_SHADOW_ALPHA_SIDE,
                DOCK_SHADOW_ALPHA_SIDE,
            ],
        });
    }
    scene.rects.push(RectInst {
        rect: card_rect,
        radius: card_radius,
        color: config.theme.background_rgba(),
        glass: 1.0,
    });

    // Dock row: per-icon magnification scales, then spread visual centers.
    // Hit-boxes stay at fixed slot positions; only drawn positions spread.
    let dock_scales: Vec<f32> = layout
        .dock_slots
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let cx = slot.x + slot.w / 2.0;
            // Hover magnification — kept exactly as the original (crisp,
            // full strength); the splash ripple is only ever added on top.
            let crest = match pointer {
                Some((px, py)) => {
                    // Below the icon, the live column reaches `dock_hit_bottom`
                    // (the screen edge while docked), so hovering the floating
                    // gap magnifies the icon just like hovering it directly.
                    let d_out = (slot.y - py).max(py - layout.dock_hit_bottom).max(0.0);
                    let fy = falloff(d_out, DOCK_MAG_VRADIUS);
                    1.0 + (dock_magnify - 1.0)
                        * falloff(px - cx, DOCK_MAG_RADIUS)
                        * fy
                        * mag_amount.clamp(0.0, 1.0)
                }
                None => 1.0,
            };
            crest + dock_ripple.get(i).copied().unwrap_or(0.0)
        })
        .collect();
    // Each magnified icon widens its visual slot by the extra pixels it
    // grew, pushing neighbours apart. Row stays centered as a whole.
    let dock_vcx: Vec<f32> = {
        let total_vw: f32 = dock_scales
            .iter()
            .map(|&s| dock_slot + dock_icon * (s - 1.0))
            .sum();
        let mut x = (w - total_vw) / 2.0;
        let mut centers = Vec::with_capacity(dock_scales.len());
        for &s in &dock_scales {
            let vw = dock_slot + dock_icon * (s - 1.0);
            centers.push(x + vw / 2.0);
            x += vw;
        }
        centers
    };
    // Divider between the pinned zone and the running-but-unpinned zone
    // (macOS): a short, faint vertical line centered between the two icons.
    if let Some(div) = dock_divider {
        if div >= 1 && div < layout.dock_slots.len() {
            if let (Some(&left), Some(&right), Some(slot)) = (
                dock_vcx.get(div - 1),
                dock_vcx.get(div),
                layout.dock_slots.get(div),
            ) {
                let inset = slot.h * 0.22;
                let fg = config.theme.text_rgba();
                scene.rects.push(RectInst {
                    rect: Rect::new(
                        (left + right) / 2.0 - DOCK_DIVIDER_W / 2.0,
                        slot.y + inset,
                        DOCK_DIVIDER_W,
                        (slot.h - inset * 2.0).max(1.0),
                    ),
                    radius: DOCK_DIVIDER_W / 2.0,
                    color: [fg[0], fg[1], fg[2], 0.22],
                    glass: 0.0,
                });
            }
        }
    }
    // Hover highlight (suppressed while dragging).
    if drag.is_none() {
        if let Some(Hit::DockIcon(i)) = hover {
            if let (Some(slot), Some(&vcx)) = (layout.dock_slots.get(i), dock_vcx.get(i)) {
                scene.rects.push(RectInst {
                    rect: Rect::new(vcx - dock_slot / 2.0, slot.y, dock_slot, slot.h),
                    radius: 12.0,
                    color: config.theme.highlight_rgba(),
                    glass: 0.0,
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
        let vcx = dock_vcx[slot] + dock_slide.get(slot).copied().unwrap_or(0.0) * dock_slot;
        let baseline = slot_rect.y + slot_rect.h;
        // Running indicator (macOS dot): a small dot beneath the icon.
        // Drawn before the icon so a magnified icon never hides it.
        if dock_running.get(slot).copied().unwrap_or(false) {
            let fg = config.theme.text_rgba();
            scene.rects.push(RectInst {
                rect: Rect::new(
                    vcx - DOCK_DOT_R,
                    baseline - DOCK_DOT_GAP - DOCK_DOT_R,
                    DOCK_DOT_R * 2.0,
                    DOCK_DOT_R * 2.0,
                ),
                radius: DOCK_DOT_R,
                color: [fg[0], fg[1], fg[2], 0.55],
                glass: 0.0,
            });
        }
        let scale = dock_scales[slot];
        let size = (dock_icon * scale).min(slot_rect.h.min(dock_slot) + MAGNIFY_HEADROOM);
        // AGUA: the icon elongates/compresses vertically about its
        // baseline with the card's motion — a water drop on the bar
        // (amplified: at icon scale the raw factor reads as nothing).
        let drop = 1.0 + (stretch - 1.0) * DOCK_STRETCH_GAIN;
        let icon_rect = Rect::new(
            vcx - size / 2.0,
            baseline - (size + lift(entry_idx)) * drop,
            size,
            size * drop,
        );
        // A drag hovering this icon's center (a fold target) rings it.
        if drag.and_then(|d| d.over_dock) == Some(slot) {
            let hl = config.theme.highlight_rgba();
            scene.rects.push(RectInst {
                rect: Rect::new(
                    icon_rect.x - 4.0,
                    icon_rect.y - 4.0,
                    icon_rect.w + 8.0,
                    icon_rect.h + 8.0,
                ),
                radius: 14.0,
                color: [hl[0], hl[1], hl[2], (hl[3] * 2.2).min(0.5)],
                glass: 0.0,
            });
        }
        if let Some((_, minis)) = group_minis.iter().find(|(e, _)| *e == entry_idx) {
            // A pinned box: a folder tile with a 2×2 mini preview.
            let hl = config.theme.highlight_rgba();
            scene.rects.push(RectInst {
                rect: icon_rect,
                radius: size * 0.22,
                color: [hl[0], hl[1], hl[2], (hl[3] * 4.0).min(0.72)],
                glass: 0.5,
            });
            let (cx, cy) = (icon_rect.x + size / 2.0, icon_rect.y + size / 2.0);
            let m = size * 0.40;
            let gap = size * 0.08;
            for (k, layer) in minis.iter().enumerate() {
                let Some(layer) = layer else { continue };
                let (col, row) = ((k % 2) as f32, (k / 2) as f32);
                scene.icons.push(IconInst {
                    rect: Rect::new(
                        cx - m - gap / 2.0 + col * (m + gap),
                        cy - m - gap / 2.0 + row * (m + gap),
                        m,
                        m,
                    ),
                    layer: *layer,
                });
            }
            continue;
        }
        scene.icons.push(IconInst {
            rect: icon_rect,
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
    // Hover tooltip: the hovered dock icon's app name in a pill above it
    // (dock icons carry no label of their own, unlike grid cells). Gated
    // behind a hover dwell by the caller (`dock_tooltip`).
    if drag.is_none() {
        if let Some(i) = dock_tooltip {
            if let (Some(&entry_idx), Some(slot), Some(&vcx)) =
                (dock_order.get(i), layout.dock_slots.get(i), dock_vcx.get(i))
            {
                if let Some(name) = entries.get(entry_idx).map(|e| e.name.clone()) {
                    let font_px = LABEL_FONT_PX;
                    let line_px = font_px * 1.3;
                    let scale = dock_scales.get(i).copied().unwrap_or(1.0);
                    let size = (dock_icon * scale).min(slot.h.min(dock_slot) + MAGNIFY_HEADROOM);
                    let icon_top = slot.y + slot.h - size - lift(entry_idx);
                    // Width from the same glyph-advance model truncate_label uses.
                    let pill_w = name.chars().count() as f32 * font_px * 0.52 + 16.0;
                    let pill_h = line_px + 6.0;
                    let pill_y = (icon_top - 6.0 - pill_h).max(2.0);
                    let mut bg = config.theme.background_rgba();
                    bg[3] = bg[3].max(0.92);
                    scene.rects.push(RectInst {
                        rect: Rect::new(vcx - pill_w / 2.0, pill_y, pill_w, pill_h),
                        radius: pill_h / 2.0,
                        color: bg,
                        glass: 0.0,
                    });
                    scene.labels.push(Label {
                        text: name,
                        pos: (vcx, pill_y + (pill_h - line_px) / 2.0),
                        max_w: pill_w,
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
                glass: 0.0,
            });
        }
        // Ghost following the pointer, topmost: the dragged icon, or a
        // box's mini stack. Slightly enlarged — it's "in hand".
        let size = dock_icon * 1.25;
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
    // search box. search_expand drives the open animation (0→1). Rides
    // the card with the rest of the content, clipped to the reveal band
    // so it sinks into the card's bottom edge while closing.
    // (Built here, pushed after the section grids so `scene.grids[s]`
    // keeps indexing the sections.)
    let mut search_grid: Option<GridContent> = None;
    if layout.search_box.y >= layout.card_top + config.window.input_bar_height as f32 * icon_scale {
        let mut sgrid = GridContent {
            clip: reveal_rect,
            ..Default::default()
        };
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
        // Expanded width: the resting SEARCH_W_MIN, growing snugly with
        // the shaped query (measured, not estimated) plus caret room.
        let content_w = (query_px + 2.0 * SEARCH_PAD_X + 8.0)
            .max(SEARCH_W_MIN)
            .min(card_w - 2.0 * GRID_PAD_X);
        let sw = lerp(btn.w, content_w, search_expand);
        let draw_rect = Rect::new(cx - sw / 2.0, boxx.y, sw, SEARCH_H);
        sgrid.rects.push(RectInst {
            rect: draw_rect,
            radius: SEARCH_H / 2.0,
            color: box_color,
            glass: 0.0,
        });

        if search_expand < 0.5 {
            // Compact button: "Search" label.
            sgrid.labels.push(Label {
                text: "Search".to_string(),
                pos: (cx, boxx.y + (SEARCH_H - SEARCH_LINE_PX) / 2.0),
                max_w: btn.w,
                font_px: SEARCH_FONT_PX,
                line_px: SEARCH_LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(reveal_clip(draw_rect)),
            });
        } else {
            // Expanded: centered query (or dim placeholder) with a real
            // caret — a thin bar sitting exactly after the shaped text
            // (`query_px`, measured by the renderer, not estimated), so
            // it hugs the last glyph as the pill grows around them.
            let (text, dim) = if query.is_empty() {
                ("Search".to_string(), true)
            } else {
                (query.to_string(), false)
            };
            sgrid.labels.push(Label {
                text,
                pos: (cx, boxx.y + (SEARCH_H - SEARCH_LINE_PX) / 2.0),
                max_w: sw - 2.0 * SEARCH_PAD_X,
                font_px: SEARCH_FONT_PX,
                line_px: SEARCH_LINE_PX,
                centered: true,
                dim,
                cache: query.is_empty(),
                clip: Some(reveal_clip(draw_rect)),
            });
            // The caret rides the centered text's right edge while
            // typing — only once the morph has (nearly) landed, so it
            // can't drift during the stretch.
            if !query.is_empty() && search_expand > 0.9 {
                let caret_x = (cx + query_px / 2.0 + 3.0).min(draw_rect.x + sw - 8.0);
                let t = config.theme.text_rgba();
                sgrid.rects.push(RectInst {
                    rect: Rect::new(caret_x, boxx.y + 6.0, 1.5, SEARCH_H - 12.0),
                    radius: 0.75,
                    color: [t[0], t[1], t[2], 0.9],
                    glass: 0.0,
                });
            }
        }
        search_grid = Some(sgrid);
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
            clip: Some(reveal_rect),
        });

        let mut grid = GridContent {
            clip: reveal_clip(sec.viewport),
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
                clip: Some(reveal_rect),
            });
            scene.grids.push(grid);
            continue;
        }

        let page_w = sec.viewport.w;
        let cells_per_page = (sec.cols * sec.rows).max(1);
        let total_w = (sec.n_pages as f32 * page_w).max(1.0);
        // Static lead cells (the navigated Files ".."): pinned at the
        // viewport start; the strip beside them pages. They render in
        // their own grid so the strip can clip against their columns
        // (paging cells slide *under* the lead, not across it).
        let lead = sec.lead;
        let strip_cpp = cells_per_page.saturating_sub(lead).max(1);
        let strip_cols = sec.cols.saturating_sub(lead).max(1);
        let mut leadgrid = GridContent {
            clip: reveal_clip(sec.viewport),
            ..Default::default()
        };
        if lead > 0 {
            grid.clip = reveal_clip(Rect::new(
                sec.viewport.x + lead as f32 * grid_cell_w,
                sec.viewport.y,
                sec.viewport.w - lead as f32 * grid_cell_w,
                sec.viewport.h,
            ));
        }
        // A cell's place comes from its display index — fractional
        // while reorder-gliding, so cells ease between grid slots.
        // Pages wrap cyclically; off-screen cells cull.
        let cell_pos = |idx: f32| -> (f32, f32) {
            let i0 = idx.max(0.0).floor() as usize;
            let frac = (idx - i0 as f32).clamp(0.0, 1.0);
            let corner = |i: usize| -> (f32, f32) {
                if i < lead {
                    // A lead cell ignores the scroll entirely.
                    return (sec.viewport.x + i as f32 * grid_cell_w, sec.viewport.y);
                }
                let j = i - lead;
                let page = j / strip_cpp;
                let within = j % strip_cpp;
                let (row, col) = (within / strip_cols, lead + within % strip_cols);
                let rel0 = (page as f32 * page_w - sec.scroll).rem_euclid(total_w);
                let rel = if rel0 >= page_w { rel0 - total_w } else { rel0 };
                (
                    sec.viewport.x + rel + col as f32 * grid_cell_w,
                    sec.viewport.y + row as f32 * grid_cell_h,
                )
            };
            let (x0, y0) = corner(i0);
            if frac <= f32::EPSILON {
                return (x0, y0);
            }
            let (x1, y1) = corner(i0 + 1);
            (lerp(x0, x1, frac), lerp(y0, y1, frac))
        };
        for i in 0..sec.cells {
            let Some(&entry_idx) = visible[s].get(i) else {
                break;
            };
            let Some(entry) = entries.get(entry_idx) else {
                break;
            };
            // The drag's origin cell vanishes (its ghost is in hand); the
            // open box's own tile vanishes (the magnified box replaces it).
            if drag_hidden == Some(entry_idx) || open_box_hidden == Some(entry_idx) {
                continue;
            }
            // Lead cells draw in their own (unclipped) grid, above the
            // strip sliding beneath them.
            let g = if i < lead { &mut leadgrid } else { &mut grid };
            let di = if s == SECTION_APPS {
                apps_slide.get(i).copied().unwrap_or(i as f32)
            } else {
                i as f32
            };
            let (cell_x, cell_y) = cell_pos(di);
            if cell_x - sec.viewport.x <= -grid_cell_w || cell_x - sec.viewport.x >= page_w {
                continue;
            }
            let cell = Rect::new(cell_x, cell_y, grid_cell_w, grid_cell_h);
            if selected == Some((s, i)) {
                let hl = config.theme.highlight_rgba();
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                    radius: 14.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 1.8).min(0.4)],
                    glass: 0.0,
                });
            } else if hover == Some(Hit::GridCell(s, i)) {
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                    radius: 14.0,
                    color: config.theme.highlight_rgba(),
                    glass: 0.0,
                });
            }
            // A drag hovering a cell that would take a drop
            // (group create/add) rings it brightly.
            if drag.and_then(|d| d.over_cell) == Some((s, i)) {
                let hl = config.theme.highlight_rgba();
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0),
                    radius: 16.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 2.2).min(0.5)],
                    glass: 0.0,
                });
            }
            let cx = cell.x + cell.w / 2.0;
            let icon_cy = cell.y + grid_icon_top + grid_icon / 2.0;
            // Drop a cell's label when its *rect* overlaps the open box —
            // not just when the icon center is inside it. Labels are wider
            // than icons and sit below them, so an edge cell just outside
            // the box can still have its label reach under the box and draw
            // over it in the later text pass. (The icon itself is fine — the
            // opaque panel covers it.)
            let covered = open_box_rect.is_some_and(|b| {
                let half = (cell.w - 12.0) / 2.0;
                let ly = cell.y + grid_icon_top + grid_icon + GRID_LABEL_GAP;
                cx + half > b.x
                    && cx - half < b.x + b.w
                    && ly + LABEL_LINE_PX > b.y
                    && ly < b.y + b.h
            });
            if let Some((_, minis)) = group_minis.iter().find(|(e, _)| *e == entry_idx) {
                // Group cell: folder-style tile with a 2×2 mini
                // preview of the first members.
                let hl = config.theme.highlight_rgba();
                let tile = box_tile;
                g.rects.push(RectInst {
                    rect: Rect::new(cx - tile / 2.0, icon_cy - tile / 2.0, tile, tile),
                    radius: 14.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 4.0).min(0.72)],
                    glass: 0.5,
                });
                let m = grid_icon * 0.42;
                let gap = grid_icon * 0.10;
                for (k, layer) in minis.iter().enumerate() {
                    let Some(layer) = layer else { continue };
                    let col_k = (k % 2) as f32;
                    let row_k = (k / 2) as f32;
                    g.icons.push(IconInst {
                        rect: Rect::new(
                            cx - m - gap / 2.0 + col_k * (m + gap),
                            icon_cy - m - gap / 2.0 + row_k * (m + gap),
                            m,
                            m,
                        ),
                        layer: *layer,
                    });
                }
                if !covered {
                    g.labels.push(Label {
                        text: truncate_label(&entry.name, cell.w - 12.0, LABEL_FONT_PX),
                        pos: (cx, cell.y + grid_icon_top + grid_icon + GRID_LABEL_GAP),
                        max_w: cell.w - 12.0,
                        font_px: LABEL_FONT_PX,
                        line_px: LABEL_LINE_PX,
                        centered: true,
                        dim: false,
                        cache: true,
                        clip: None,
                    });
                }
                continue;
            }
            let scale = match pointer {
                Some((px, py)) => {
                    let d = ((px - cx).powi(2) + (py - icon_cy).powi(2)).sqrt();
                    1.0 + (GRID_MAGNIFY - 1.0)
                        * falloff(d, GRID_MAG_RADIUS)
                        * mag_amount.clamp(0.0, 1.0)
                }
                None => 1.0,
            };
            let size = grid_icon * scale;
            g.icons.push(IconInst {
                rect: Rect::new(
                    cx - size / 2.0,
                    icon_cy - size / 2.0 - lift(entry_idx),
                    size,
                    size,
                ),
                layer: layer_of(entry_idx),
            });
            if !covered && placeholders.get(entry_idx).copied().unwrap_or(false) {
                if let Some(ch) = entry.name.chars().next() {
                    let letter: String = ch.to_uppercase().collect();
                    let font_px = (size * 0.46).clamp(10.0, 30.0);
                    let line_px = font_px * 1.3;
                    g.labels.push(Label {
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
            // A profile mutation in flight swaps the name for a progress
            // note (dimmed); a failed one flashes "Failed".
            let is_busy = busy.get(entry_idx).copied().unwrap_or(false);
            let is_failed = failed.get(entry_idx).copied().unwrap_or(false);
            let is_installing = installing.get(entry_idx).copied().unwrap_or(false);
            let is_launching = launching.get(entry_idx).copied().unwrap_or(false);
            let name = if is_busy && is_launching {
                "Launching…".to_string()
            } else if is_busy && (s == SECTION_INSTALL || is_installing) {
                "Installing…".to_string()
            } else if is_busy {
                "Removing…".to_string()
            } else if is_failed {
                "Failed".to_string()
            } else {
                truncate_label(&entry.name, cell.w - 12.0, LABEL_FONT_PX)
            };
            if !covered {
                g.labels.push(Label {
                    text: name,
                    pos: (cx, cell.y + grid_icon_top + grid_icon + GRID_LABEL_GAP),
                    max_w: cell.w - 12.0,
                    font_px: LABEL_FONT_PX,
                    line_px: LABEL_LINE_PX,
                    centered: true,
                    dim: is_busy,
                    cache: true,
                    clip: None,
                });
            }
        }
        scene.grids.push(grid);
        if lead > 0 {
            scene.grids.push(leadgrid);
        }

        // Page indicator dots in the gap beneath the section — gated by
        // the reveal edge (they ride the same fixed layout as the grid,
        // so unclipped they would linger as a ghost once the box hides).
        let dot_y = sec.viewport.y + sec.viewport.h + SECTION_GAP / 2.0;
        if sec.n_pages > 1 && (reveal_top..=reveal_bottom).contains(&dot_y) {
            let dot_r = 3.0;
            let dot_spacing = 10.0;
            let total_dots_w = sec.n_pages as f32 * dot_spacing - (dot_spacing - dot_r * 2.0);
            let dot_cx = sec.viewport.x + sec.viewport.w / 2.0;
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
                    glass: 0.0,
                });
            }
        }
    }
    if let Some(g) = search_grid {
        scene.grids.push(g);
    }

    // Magnified open box: a rounded panel that grows from the clicked tile
    // (`group_origin`) to fill the Apps grid, with the member icons in a
    // big centered 3×3 and the box name as a title. Drawn last so it sits
    // opaquely over the loose grid behind it.
    if let (Some(box_rect), Some((ox, oy))) = (open_box_rect, group_origin) {
        let t = group_expand.clamp(0.0, 1.0);
        // Starts at the tile's radius for a seamless spring-out, then rounds up
        // to the shared box radius — the same corner as the fully-open main
        // card, and rounder than the macOS-style dock behind it.
        let radius = lerp(14.0, BOX_CORNER_RADIUS, t);
        // Built as its own grid pushed last, so it draws over the loose
        // grid behind it (section grids draw in order).
        let mut boxgrid = GridContent {
            clip: reveal_rect,
            ..Default::default()
        };
        // The box panel shares the same liquid-glass material as the main card
        // so it looks and feels like a smaller card: same rim glow, iridescence,
        // cursor spotlight, and live water effects, all computed relative to the
        // box's own bounds. The panel fades in with the open animation.
        // Same base opacity as the main card (glass), fading in with the open
        // animation — so a group box reads as the same colour as the main box,
        // not a darker, more opaque panel.
        let mut panel = config.theme.background_rgba();
        panel[3] *= t;
        let (bjl, bjr, bjt, bjb) = box_push;
        let box_draw = Rect::new(
            box_rect.x + bjl,
            box_rect.y + bjt,
            box_rect.w - bjl + bjr,
            box_rect.h - bjt + bjb,
        );
        // Everything from here (the box panel + its members) is the box
        // overlay: the renderer draws the base scene before it into the
        // offscreen texture, blurs the box's region, and composites this on
        // top (see `blur_split` / `box_rect`).
        scene.blur_split = Some(scene.grids.len());
        scene.box_rect = Some((box_draw, radius));
        // Same drop-shadow pattern as the dock, but only for a box floating on
        // top of the dock (dock/dir stack) — grid boxes inside the launcher
        // sit on the card and get none. Fades in with the box open (`t`).
        if box_over_dock && t > 0.001 {
            scene.shadows.push(ShadowInst {
                rect: box_draw,
                radius,
                blur: DOCK_SHADOW_BLUR,
                color: [0.0, 0.0, 0.0, t],
                edges: [
                    DOCK_SHADOW_ALPHA_TOP,
                    DOCK_SHADOW_ALPHA_BOTTOM,
                    DOCK_SHADOW_ALPHA_SIDE,
                    DOCK_SHADOW_ALPHA_SIDE,
                ],
            });
        }
        boxgrid.rects.push(RectInst {
            rect: box_draw,
            radius,
            color: panel,
            glass: 0.5,
        });
        // The 3×3 slot each member takes, by the grid side the box sits on:
        // left pins member 0 (A) top-left and fills row-major; right pins
        // member 1 (B) top-right. The rest keep their drag order.
        let vp = layout.sections[SECTION_APPS].viewport;
        let member_slot: [usize; 9] = if ox < vp.x + vp.w / 2.0 {
            // Left: A pinned top-left; D takes top-right, C the middle-left.
            [0, 1, 3, 2, 4, 5, 6, 7, 8]
        } else {
            // Right: B pinned top-right; C takes top-left, D the middle-right.
            [1, 2, 0, 5, 3, 4, 6, 7, 8]
        };
        let cell = box_rect.w / OPEN_BOX_COLS as f32;
        // Closed 2×2 mini geometry (matches the closed tile), centered on the
        // tile — the preview four flow from here into their open slots.
        let mini = grid_icon * 0.42;
        let mini_offset = (mini + grid_icon * 0.10) / 2.0;
        // The extra members reveal as the box unfolds past the 2×2.
        let reveal = {
            let u = ((t - 0.15) / 0.55).clamp(0.0, 1.0);
            u * u * (3.0 - 2.0 * u)
        };
        // Members lay out in a horizontal strip of 3×3 pages shifted by
        // `box_scroll`, so paging slides. Everything clips to the box.
        let page_w = layout.open_box.map_or(box_rect.w, |b| b.w);
        boxgrid.clip = box_rect;
        // Members draw at their animated display slot (fractional
        // mid-glide — the reflow itself is computed in `draw`, shared
        // with the grid): positions lerp between the cell centers of the
        // two neighboring slots, pages included.
        let place = |di: usize| -> (f32, f32) {
            let slot = member_slot[di % OPEN_BOX_CAP];
            let (row, col) = (slot / OPEN_BOX_COLS, slot % OPEN_BOX_COLS);
            let page_x = (di / OPEN_BOX_CAP) as f32 * page_w - box_scroll;
            (
                box_rect.x + page_x + (col as f32 + 0.5) * cell,
                box_rect.y + (row as f32 + 0.5) * cell,
            )
        };
        for (m, &idx) in open_box_members.iter().enumerate() {
            if box_drag.is_some_and(|(h, _, _)| h == idx) {
                continue; // dragged member: a ghost, not in the grid
            }
            let d = open_box_disp.get(m).copied().unwrap_or(m as f32);
            let d0 = d.max(0.0).floor() as usize;
            let frac = (d - d0 as f32).clamp(0.0, 1.0);
            let (mut open_cx, mut open_cy) = place(d0);
            if frac > f32::EPSILON {
                let (x1, y1) = place(d0 + 1);
                open_cx = lerp(open_cx, x1, frac);
                open_cy = lerp(open_cy, y1, frac);
            }
            // Cull members more than a page off either side of the box.
            if open_cx < box_rect.x - cell || open_cx > box_rect.x + box_rect.w + cell {
                continue;
            }
            let (cx, cy, size) = if frac <= f32::EPSILON && d0 < 4 {
                // Page-0 preview icons flow from the closed 2×2 into place.
                let sx = if d0 == 0 || d0 == 2 { -1.0 } else { 1.0 };
                let sy = if d0 < 2 { -1.0 } else { 1.0 };
                (
                    lerp(ox + sx * mini_offset, open_cx, t),
                    lerp(oy + sy * mini_offset, open_cy, t),
                    lerp(mini, grid_icon, t),
                )
            } else {
                (open_cx, open_cy, lerp(mini, grid_icon, t) * reveal)
            };
            if size <= 0.5 {
                continue;
            }
            boxgrid.icons.push(IconInst {
                rect: Rect::new(cx - size / 2.0, cy - size / 2.0, size, size),
                layer: layer_of(idx),
            });
            if placeholders.get(idx).copied().unwrap_or(false) {
                if let Some(ch) = entries.get(idx).and_then(|e| e.name.chars().next()) {
                    let font_px = (size * 0.46).clamp(8.0, 30.0);
                    boxgrid.labels.push(Label {
                        text: ch.to_uppercase().collect(),
                        pos: (cx, cy - font_px * 1.3 / 2.0),
                        max_w: size,
                        font_px,
                        line_px: font_px * 1.3,
                        centered: true,
                        dim: false,
                        cache: true,
                        clip: Some(box_rect),
                    });
                }
            }
            // Member names pop in near the end of the open, below each icon.
            if t > 0.85 {
                if let Some(entry) = entries.get(idx) {
                    boxgrid.labels.push(Label {
                        text: truncate_label(&entry.name, cell - 8.0, LABEL_FONT_PX),
                        pos: (cx, cy + size / 2.0 + 3.0),
                        max_w: cell - 8.0,
                        font_px: LABEL_FONT_PX,
                        line_px: LABEL_LINE_PX,
                        centered: true,
                        dim: false,
                        cache: true,
                        clip: Some(box_rect),
                    });
                }
            }
        }
        scene.grids.push(boxgrid);
        // Page dots beneath the box (own grid so they aren't clipped to it).
        let (page, pages) = open_box_pages;
        if pages > 1 && t > 0.6 {
            let mut dots = GridContent {
                clip: reveal_rect,
                ..Default::default()
            };
            let dot_r = 3.0;
            let spacing = 12.0;
            let total_w = pages as f32 * spacing - (spacing - dot_r * 2.0);
            let dy = box_rect.y + box_rect.h + 12.0;
            let x0 = box_rect.x + box_rect.w / 2.0 - total_w / 2.0;
            let hl = config.theme.highlight_rgba();
            for p in 0..pages {
                let x = x0 + p as f32 * spacing + dot_r;
                let a = if p == page { hl[3] } else { hl[3] * 0.35 };
                dots.rects.push(RectInst {
                    rect: Rect::new(x - dot_r, dy - dot_r, dot_r * 2.0, dot_r * 2.0),
                    radius: dot_r,
                    color: [hl[0], hl[1], hl[2], a],
                    glass: 0.0,
                });
            }
            scene.grids.push(dots);
        }
    }

    // Ghost of a box member being reordered, following the pointer (topmost).
    if let Some((entry, _, pos)) = box_drag {
        let s = grid_icon * 1.1;
        scene.overlay.push(IconInst {
            rect: Rect::new(pos.0 - s / 2.0, pos.1 - s / 2.0, s, s),
            layer: layer_of(entry),
        });
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
                startup_wm_class: None,
                needs_terminal: false,
                path: None,
            })
            .collect()
    }

    fn vis(n: usize) -> [Vec<usize>; N_SECTIONS] {
        [(0..n).collect(), Vec::new(), Vec::new()]
    }

    /// Identity display slots (a dense grid: slot == list index).
    fn idslots(n: usize) -> Vec<usize> {
        (0..n).collect()
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
            1.0,
            SURFACE,
            OPEN,
            n,
            [n, 0, 0],
            [scroll, 0.0, 0.0],
            [false; N_SECTIONS],
            (1.0, 1.0),
        )
    }

    #[test]
    fn docked_extent_shows_dock_row_only() {
        let cfg = config();
        let l = layout(
            &cfg,
            1.0,
            SURFACE,
            48.0,
            20,
            [20, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
            (1.0, 1.0),
        );
        assert!(!l.dock_slots.is_empty());
        // Content sits at its fixed open position; while docked the
        // reveal region (below the dock band) is empty, so every grid
        // clips to nothing and no cells show.
        let s = scene(
            &cfg,
            1.0,
            &l,
            &entries(20),
            &vis(20),
            SURFACE,
            &FrameInput {
                alpha: 1.0,
                ..Default::default()
            },
        );
        assert!(s.grids.iter().all(|g| g.clip.h <= 0.0));
    }

    #[test]
    fn open_extent_shows_three_sections() {
        let cfg = config();
        let l = open_layout(&cfg, 40, 1e9);
        let apps = &l.sections[SECTION_APPS];
        assert_eq!(apps.cols, 6, "720px card fits exactly 6 columns");
        assert_eq!(apps.rows, 4, "tightened gaps let the default card fit 6×4");
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
            1.0,
            &l,
            &entries(40),
            &vis(40),
            SURFACE,
            &FrameInput {
                alpha: 1.0,
                ..Default::default()
            },
        );
        // The section grids lead (indexable by section); the clipped
        // search-widget grid follows them.
        assert!(s.grids.len() >= N_SECTIONS);
        let apps_grid = &s.grids[SECTION_APPS];
        assert!(!apps_grid.icons.is_empty());
        assert_eq!(apps_grid.icons.len(), apps_grid.labels.len());
    }

    #[test]
    fn files_section_shows_its_own_entries() {
        let cfg = config();
        let l = layout(
            &cfg,
            1.0,
            SURFACE,
            OPEN,
            10,
            [4, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
            (1.0, 1.0),
        );
        let visible = [vec![0, 1, 2, 3], Vec::new(), vec![4, 5, 6, 7, 8, 9]];
        let s = scene(
            &cfg,
            1.0,
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
            1.0,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
            (1.0, 1.0),
        );
        let slot = l.dock_slots[0];
        assert_eq!(
            hit_test(&l, (slot.x + 1.0, slot.y + 1.0), false, &idslots(10)),
            Some(Hit::DockIcon(0))
        );
        // Second cell of the apps section's first row.
        let apps = &l.sections[SECTION_APPS];
        let cell1 = (
            apps.viewport.x + GRID_CELL_W * 1.5,
            apps.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(
            hit_test(&l, cell1, false, &idslots(10)),
            Some(Hit::GridCell(SECTION_APPS, 1))
        );
        // First cell of the files section.
        let files = &l.sections[SECTION_FILES];
        let fcell = (
            files.viewport.x + GRID_CELL_W / 2.0,
            files.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(
            hit_test(&l, fcell, false, &idslots(10)),
            Some(Hit::GridCell(SECTION_FILES, 0))
        );
        // The empty install section hits nothing.
        let install = &l.sections[SECTION_INSTALL];
        let icell = (
            install.viewport.x + GRID_CELL_W / 2.0,
            install.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(hit_test(&l, icell, false, &idslots(10)), None);
        assert_eq!(hit_test(&l, (1.0, 1.0), false, &idslots(10)), None);
    }

    #[test]
    fn navigation_does_not_shift_the_files_title() {
        // Browsing exits through the ".." tile (the listing's first
        // cell), not a Back pill — the title stays put either way.
        let cfg = config();
        let flat = layout(
            &cfg,
            1.0,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
            (1.0, 1.0),
        );
        let nav = layout(
            &cfg,
            1.0,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false, false, true],
            (1.0, 1.0),
        );
        assert_eq!(
            nav.sections[SECTION_FILES].title_pos,
            flat.sections[SECTION_FILES].title_pos
        );
    }

    #[test]
    fn section_at_routes_scroll_bands() {
        let cfg = config();
        let l = layout(
            &cfg,
            1.0,
            SURFACE,
            OPEN,
            10,
            [10, 0, 6],
            [0.0; N_SECTIONS],
            [false; N_SECTIONS],
            (1.0, 1.0),
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
            hit_test(&l, pos, false, &idslots(18 * 3)),
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
            assert_eq!(hit_test(&l, pos, false, &idslots(7)), None);
        }
    }
}

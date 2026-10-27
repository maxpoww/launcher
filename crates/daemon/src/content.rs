//! Card content layout: where the dock icon row and the app-grid cells
//! sit for a given animation extent, pointer hit-testing against them,
//! and assembly of the frame's draw scene. Pure math — no Wayland, no
//! wgpu — so it stays unit-testable and is shared by rendering and
//! input handling.

use waverunner_core::index::AppEntry;
use waverunner_core::Config;

use crate::launcher_prefs::Side;

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

    /// What two rectangles share (empty — zero-sized — when nothing).
    pub fn intersect(&self, o: &Rect) -> Rect {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
}

/// THE WINDOWS' CORNER, as a [`RectInst::radius`]: the curve Golem's windows
/// are rounded with (hyprland.lua: `rounding = 14`, `rounding_power = 3` —
/// chosen by eye on the live screen, Max 2026-10-12: *"me gusta la curva…
/// usa esa curva en la tarjeta"*; a circle of 12 read as "too square", power
/// 4 showed "una ligera punta").
///
/// 21, NOT 14: Hyprland draws a window's corner at `rounding × power / 2`
/// ("make perceived roundness consistent", `CWindow::rounding`), so 14 at
/// power 3 is a superellipse of 21. Measured on a real window, pixel row by
/// pixel row; at 14 the card's corner was visibly tighter than its window's
/// (Max: *"no parece la misma curva… las esquinas se ven diferentes"*).
///
/// NEGATIVE on purpose: the sign is what tells `rounded_rect.wgsl` to draw
/// that curve (its `WINDOW_POWER`) and not a circle's quarter. Change the
/// config, this and the shader together.
pub(crate) const WINDOW_CORNER: f32 = -(14.0 * 3.0 / 2.0);

/// One rounded rectangle to fill (card background, hover highlight).
#[derive(Debug, Clone, Copy)]
pub struct RectInst {
    pub rect: Rect,
    /// The corner: a circle's quarter of this radius — or, NEGATIVE, the
    /// windows' own curve of that size ([`WINDOW_CORNER`]).
    pub radius: f32,
    pub color: [f32; 4],
    /// Glass-material strength: `0.0` → plain solid fill; `1.0` → full
    /// liquid-glass material. Values in between keep all glass layers at
    /// full strength except the iridescent tint, which is scaled by this
    /// factor — so `0.5` gives the same rim glow / spotlight as `1.0` but
    /// half the rainbow shimmer (used for the group-box panel, which is
    /// smaller so edge effects already read more strongly).
    pub glass: f32,
    /// Stroke width in logical px: `0.0` fills the shape, anything larger
    /// draws only a band of that thickness just inside the edge — a real
    /// rounded outline, corners included, which is why the hovered row's
    /// frame is one instance rather than four thin rects.
    pub border: f32,
}

/// One icon quad, referencing a texture-array layer.
#[derive(Debug, Clone, Copy)]
pub struct IconInst {
    pub rect: Rect,
    pub layer: u32,
    /// Colour wash over the icon's silhouette: `rgb` the colour, `a` the
    /// strength (0 = untinted, the default). Used to flush the drag ghost
    /// red while it hovers the uninstall target — the icon that's about to
    /// disappear literally reddens in hand.
    pub tint: [f32; 4],
    /// Progress-ring mode: `0.0..=1.0` makes this quad a circular install
    /// progress ring (that fraction filled) instead of a textured icon; a
    /// negative value (the default) draws the icon normally.
    pub ring: f32,
    /// Squircle plate drawn *in the shader* under the glyph (rgba;
    /// `a <= 0` = no plate). The plate used to be baked white into the
    /// icon texture cache; drawing it live lets it take the surface's
    /// adaptive colour and ride the same eases as everything else, at the
    /// cost of two SDFs per icon pixel. [`NO_PLATE`] for image thumbnails
    /// and special-mode quads (rings, shine, the bin); [`PLATE_STATIC`]
    /// keeps the old baked look on static surfaces (notif card).
    pub plate: [f32; 4],
}

/// Set on an `IconInst::layer`: the layer holds a PICTURE (a photo's, a
/// video's thumbnail), not an icon — the palette's icon filter leaves it as
/// it is, and no squircle cuts it. For the surfaces whose array mixes both
/// (Files, the desktop: no `thumb_base` block); the shader masks the bit off.
pub const PICTURE: u32 = 1 << 31;

/// Set on an `IconInst::layer` beside [`PICTURE`]: the picture is a
/// WALLPAPER's tile, which the shader grades with the Colours slider's live
/// level (`wall_tint`). The shader masks it off too.
pub const WALL: u32 = 1 << 30;

/// No plate under this icon quad (image thumbnails, ring/shine overlays).
pub const NO_PLATE: [f32; 4] = [0.0; 4];

/// A `RectInst::glass` for a solid fill the night light leaves alone (no
/// line of light along its top, no shade toward its bottom): a window's own
/// ground, as Golem Files' sidebar and i card (`rounded_rect.wgsl`).
pub const UNLIT: f32 = -0.3;
/// The old baked plate's exact look — translucent white frost (84/255).
/// Static surfaces that don't colour-match keep this.
pub const PLATE_STATIC: [f32; 4] = [1.0, 1.0, 1.0, 0.33];

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

/// Sentinel `Label::family` meaning "the default sans, BOLD".
///
/// Weight rides the family field rather than a new one: `Label` is built at
/// ~45 sites, and threading a `bold` flag through all of them to serve two
/// hover states would be noise. The renderer resolves this name specially
/// (there is no font actually called it — the `\u{1}` prefix guarantees no
/// real family can collide).
pub const FONT_BOLD: &str = "\u{1}bold";

/// The same for MEDIUM (500): a touch heavier than the words around it
/// (the Files pill's name). A sans with no medium face falls back to its
/// regular one, never to bold.
pub const FONT_MEDIUM: &str = "\u{1}medium";

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
    /// Font family override (`None` = the default SansSerif). OPTIONS labels
    /// set this to a Nerd Font for its icons + text.
    pub family: Option<&'static str>,
    /// Per-label colour override (`None` = the frame's default text colour).
    /// Used for e.g. the OPTIONS red close glyph.
    pub color: Option<[f32; 4]>,
}

/// Scrollable grid content, clipped to `clip` by the renderer.
#[derive(Debug, Default)]
pub struct GridContent {
    pub clip: Rect,
    pub rects: Vec<RectInst>,
    pub icons: Vec<IconInst>,
    pub labels: Vec<Label>,
    /// Opacity of `icons`; none = whole (the dock's edge pins and Apps
    /// button wear the Icons section's "Dock icons").
    pub fade: Option<f32>,
}

/// Everything the renderer draws for one frame, in draw order.
#[derive(Debug, Default)]
pub struct Scene {
    pub alpha: f32,
    /// Edge shadows, drawn first (behind everything).
    pub shadows: Vec<ShadowInst>,
    /// Edge shadows drawn *over* the unclipped fills (not behind) — for
    /// neumorphic button depth on the OPTIONS bar, where the opaque bar fill
    /// would otherwise cover a behind-shadow.
    pub overlay_shadows: Vec<ShadowInst>,
    /// Unclipped fills: card background first, then dock hover and the
    /// search box.
    pub rects: Vec<RectInst>,
    /// Dock icon quads (unclipped; they live in the card's top sliver).
    pub icons: Vec<IconInst>,
    /// Turns (radians) of `icons`, by index; missing = none (the bin's lid
    /// and can, `bin_motion`).
    pub icon_rot: Vec<f32>,
    /// Opacity of `icons`, by index; missing = whole (the dock's icons wear
    /// the Icons section's "Dock icons").
    pub icon_fade: Vec<f32>,
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
    /// Banner blister: `(bar_edge_y, k, _, _)` for the sunset module's swell.
    /// `None` = no blister this frame.
    pub neck: Option<[f32; 4]>,
    /// A MOVING PICTURE (Quick Look's video): one quad over the renderer's
    /// video texture (`Renderer::set_video_frame`; its `layer` is ignored),
    /// drawn right after grid `.0`'s pictures — over that grid's frame,
    /// under the grids that follow (its controls).
    pub video: Option<(usize, IconInst)>,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    DockIcon(usize),
    /// A cell in one of the popup sections: (section, cell index into
    /// that section's visible list).
    GridCell(usize, usize),
    SearchButton,
    /// A member cell of the magnified open box: the 3×3 slot index under
    /// the pointer (0..9). Clicking a filled slot launches it; dragging
    /// pulls the app out of the box. A click on an empty slot is inert —
    /// only a click *outside* the box closes it. Also used for a dock
    /// folder's stack (the same box, anchored above the dock).
    OpenBoxCell(usize),
    /// A pill in the Controls row (index into `SearchState::controls`):
    /// opens that setting on the control panel.
    Control(usize),
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
const SECTION_TITLE_H: f32 = 5.0;
/// The spacing of Max's framed-sections mockup (2026-09-30), kept without
/// the frames ("only the positions"): the gap between one section and the
/// next, the room under a grid where its page dots sit, and the air kept
/// under the last section's dots. (Then the grids and dots 4 px closer to
/// their titles, the titles staying put: the band 14 -> 10, the gap
/// 20 -> 24; then 3 px more: 7 and 27; then 2 more: 5 and 29.)
const SECTION_GAP: f32 = 29.0;
const DOT_ROOM: f32 = 6.0;
/// How much further below its title the Controls row sits than icons do.
const CONTROLS_EXTRA_DROP: f32 = 8.0;
/// The page dots sit this much above the middle of their room (Max: "the
/// dots slightly higher").
const DOTS_RAISE: f32 = 1.5;
const LAST_SECTION_AIR: f32 = 6.0;

/// Fixed layout metrics (logical px). Config-independent for now; can
/// move into `[theme]` if tuning is wanted.
pub const DOCK_ICON: f32 = 40.0;
pub const DOCK_SLOT: f32 = 44.0;
pub const DOCK_PAD_X: f32 = 10.0;
/// Breathing room under the dock icons: the icon baseline floats this many
/// px above the slot bottom instead of sitting ON it (Max, 2026-09-10: the
/// icons sat flush on the bar's bottom edge, gap only above). With the
/// 46px bar (`input_bar_height`) wrapping the 44px slot, this lands the
/// 40px icon with ~3px of card showing above and below it.
const DOCK_BASELINE_PAD: f32 = 2.0;
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
/// The open box's page is nine members (a stored 3×3 from the square box
/// it was; now laid in rows over the tile — see [`row_box`]). Also the
/// rows the Apps grid keeps while a box is open.
pub const OPEN_BOX_COLS: usize = 3;
/// Members per open-box page — the box grid's capacity, and the page
/// size of a group's member list (`groups::PAGE_CAP` re-exports it).
pub const OPEN_BOX_CAP: usize = OPEN_BOX_COLS * OPEN_BOX_COLS;
const GRID_PAD_X: f32 = 14.0;
const GRID_TOP_GAP: f32 = 13.0;
const GRID_BOTTOM_PAD: f32 = 6.0;
/// How far the search pill lifts off the card's floor into spare room above
/// it — never closer than [`SEARCH_LIFT_AIR`] to the Files row, so it never
/// costs the grid a row (Max, 2026-09-29: "move the search up a little").
const SEARCH_LIFT: f32 = 8.0;
const SEARCH_LIFT_AIR: f32 = 4.0;
/// Text metrics for the app-name labels.
pub const LABEL_FONT_PX: f32 = 12.0;
pub const LABEL_LINE_PX: f32 = 16.0;
/// Section titles (Apps, Install, Files, Controls), from Max's mockup
/// (2026-09-30): small and bold, faint, raised a little in their band.
pub(crate) const TITLE_FONT_PX: f32 = 13.0;
const TITLE_LINE_PX: f32 = 16.0;
pub(crate) const TITLE_ALPHA: f32 = 0.3;
const TITLE_DROP: f32 = -1.0;
/// Search box: height, inner padding, text metrics.
const SEARCH_H: f32 = 28.0;
/// Width of the collapsed "Filter" button pill.
const SEARCH_BTN_W: f32 = 80.0;
/// Minimum expanded width — the resting size; the pill grows from here
/// with the shaped query.
const SEARCH_W_MIN: f32 = 160.0;
const SEARCH_GAP: f32 = 7.0;

/// The search pill as one more OPTIONS pill (Max, 2026-09-30: "make the
/// search pill look and feel like the other pills"): the bar pill's height
/// and type — the control panel's middle layer — at the bar's scale.
#[derive(Clone, Copy, Debug)]
pub struct SearchPill {
    /// The OPTIONS scale (`App::options_scale`).
    pub scale: f32,
    /// The bar pill's height (`App::options_pill_h`).
    pub h: f32,
    /// Shaped width of the "Search" label at the pill's font.
    pub label_px: f32,
    /// Eased pointer hover, 0..1.
    pub lift: f32,
}

impl Default for SearchPill {
    fn default() -> Self {
        Self { scale: 1.0, h: 23.0, label_px: 52.0, lift: 0.0 }
    }
}

/// What THE DEEP FILE SEARCH draws (`deep_search.rs`): its shelves' titles
/// (the three sections', retitled: "PDF · 4"; a hidden one "" — its room is
/// the first's, [`fold_shelves`]), a line under each file's name (when it
/// was used), the files picked to act on together, its TABS' words (under
/// the field: `Layout::tabs_row`, their rects `Layout::controls`) and the
/// ones lit, and the chip in the search pill ("Files", "Files › Facturas",
/// with its shaped width).
#[derive(Clone, Copy, Debug)]
pub struct DeepDraw<'a> {
    pub titles: [&'a str; N_SECTIONS],
    pub hidden: [bool; N_SECTIONS],
    pub subs: &'a std::collections::HashMap<usize, String>,
    pub marked: &'a [usize],
    pub tabs: &'a [String],
    pub lit: &'a [usize],
    pub chip: &'a str,
    pub chip_w: f32,
    /// The shaped width of the pill's placeholder ("Search files").
    pub placeholder_w: f32,
}

/// How far the chip in the search pill stands from the words typed after it.
const PILL_CHIP_GAP: f32 = 8.0;

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

/// How much of the theme's card translucency the RESTING dock keeps (Max,
/// 2026-09-10: "more transparent dock — not the icons, and not the box").
/// The card fill's alpha is scaled by this while docked, easing back to the
/// theme's full alpha as `card_open` rises — so calling the box gradually
/// restores the current look. Note the docked alpha lands *under* the
/// compositor's `ignore_alpha` threshold (0.5, hyprland.lua), so the
/// resting card reads as CLEAR glass — crisp see-through, no layer blur —
/// with the frost arriving as the fill crosses the threshold mid-open.
pub(crate) const DOCK_REST_FILL: f32 = 0.65;

/// FAST LAUNCH's card: the theme's card translucency it keeps — the
/// theme's 0.502 → 0.49, the densest under the `ignore_alpha` threshold
/// (0.5): clear glass, no blur.
const FAST_FILL: f32 = 0.97;

/// Dock drop shadow. One continuous soft shadow wraps the whole rounded
/// dock; `BLUR` is how far the penumbra reaches out from every edge, and the
/// four `ALPHA`s set per-edge darkness (corners blend the two neighbours, so
/// there are no seams). Alphas stay under the compositor's `ignore_alpha`
/// blur threshold so the shadow never triggers layer blur.
pub(crate) const DOCK_SHADOW_BLUR: f32 = 16.0;
pub(crate) const DOCK_SHADOW_ALPHA_TOP: f32 = 0.10;
pub(crate) const DOCK_SHADOW_ALPHA_BOTTOM: f32 = 0.40;
pub(crate) const DOCK_SHADOW_ALPHA_SIDE: f32 = 0.10;
/// On a near-white wallpaper (the toned glass) the shadow round the top and
/// the sides is this much lighter; the bottom one stays (Max, 2026-10-23).
pub(crate) const DOCK_SHADOW_TONED_CUT: f32 = 0.35;
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
/// Peak scale of a dock icon directly under the cursor. Was 1.5;
/// softened in steps (Max, 2026-09-10: "make the magnification on hover
/// subtler") to 1.25 — a gentle rise, not a lunge.
pub(crate) const DOCK_MAGNIFY: f32 = 1.25;
/// Horizontal falloff radius of dock magnification, in pixels.
pub(crate) const DOCK_MAG_RADIUS: f32 = 120.0;
/// Vertical attenuation radius, measured from the dock band's edges
/// (zero inside the band). Small on purpose: the effect must die out
/// before the first grid row so hovering the grid never stirs the dock.
pub(crate) const DOCK_MAG_VRADIUS: f32 = 28.0;
/// Temporal smoothing of the dock magnification (exponential approach,
/// s⁻¹). The pointer only sets *targets*; each icon's drawn scale eases
/// toward its target — quickly while growing (the crest stays tight
/// under the cursor) and more gently while shrinking (icons melt back
/// instead of snapping) — so entering, sweeping across, and leaving the
/// dock all read as one liquid motion rather than per-event jumps.
pub(crate) const DOCK_MAG_GROW_RATE: f32 = 36.0;
pub(crate) const DOCK_MAG_RELEASE_RATE: f32 = 18.0;
/// Settle threshold for the smoothed scales (scale units).
pub(crate) const DOCK_MAG_SNAP: f32 = 0.002;
/// How much of the magnified row's spread the CARD takes. The magnified
/// icons part to make room and the card widens out of that same motion —
/// same curve, same moment, because both read the one smoothed
/// `dock_mag` — but at a shorter reach, so the bar breathes with the row
/// rather than tracking it pixel for pixel. `1.0` welds them edge to edge;
/// `0.0` leaves the card perfectly still under a moving row.
///
/// Tuned by feel, live: 1.0 (welded, ~13.5px a side) → 0.5 → 0.25 → 0.15
/// → back to **0.25** (~3.4px a side at the default magnification), which
/// is where it landed: 0.15 was too little to read as the bar giving with
/// the row. Whatever the value, the card's motion stays the row's own —
/// only its amplitude changes.
pub(crate) const DOCK_SPREAD_FOLLOW: f32 = 0.25;
/// Running-indicator dot radius (px) and its gap below the icon baseline.
// The dock's dots (running apps, the apps button's held pins): a little
// lower, a little bigger, WHITE so they read (Max, 2026-10-16).
const DOCK_DOT_R: f32 = 2.4;
const DOCK_DOT_GAP: f32 = 1.0;
const DOCK_DOT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.9];

/// A row of `n` dots centred on `cx` (each `DOCK_DOT_R`, three radii apart,
/// drawn closer when that would pass `max_w`) with their bottoms on `y`.
fn dock_dots(rects: &mut Vec<RectInst>, cx: f32, y: f32, n: usize, max_w: f32) {
    if n == 0 {
        return;
    }
    let step = (DOCK_DOT_R * 3.0).min(max_w / n as f32);
    let first = cx - step * (n as f32 - 1.0) / 2.0;
    for k in 0..n {
        let x = first + step * k as f32;
        rects.push(RectInst {
            rect: Rect::new(x - DOCK_DOT_R, y - 2.0 * DOCK_DOT_R, DOCK_DOT_R * 2.0, DOCK_DOT_R * 2.0),
            radius: DOCK_DOT_R,
            color: DOCK_DOT_COLOR,
            glass: 0.0,
            border: 0.0,
        });
    }
}
/// [`dock_dots`] beside a side dock's icon: a column about `cy`, each dot
/// centred on `x` (the icon's edge side, as under a bottom dock's icon).
fn dock_dots_col(rects: &mut Vec<RectInst>, x: f32, cy: f32, n: usize, max_h: f32) {
    if n == 0 {
        return;
    }
    let step = (DOCK_DOT_R * 3.0).min(max_h / n as f32);
    let first = cy - step * (n as f32 - 1.0) / 2.0;
    for k in 0..n {
        let y = first + step * k as f32;
        rects.push(RectInst {
            rect: Rect::new(x - DOCK_DOT_R, y - DOCK_DOT_R, DOCK_DOT_R * 2.0, DOCK_DOT_R * 2.0),
            radius: DOCK_DOT_R,
            color: DOCK_DOT_COLOR,
            glass: 0.0,
            border: 0.0,
        });
    }
}
/// Width (px) of the divider between the pinned and running-unpinned zones.
const DOCK_DIVIDER_W: f32 = 1.5;
/// Extra air before the minimized-window tiles, setting them off from the
/// pinned/trash icons as their own group (Max, 2026-09-16). Base px, scaled
/// by `icon_scale` at use.
// Sized so the divider sits MIN_TILE_PAD past the trash and the first
// thumbnail sits MIN_TILE_PAD past the divider — i.e. the divider↔thumbnail
// gap equals the thumbnail↔thumbnail gap (Max, 2026-09-16: "make the space
// between the thumbnail and the divider the same as between the thumbnails").
// The thumbnail zone starts DOCK_MIN_GAP past the pinned zone; a tile's
// picture sits MIN_TILE_PAD/2 into its slot, so 1.5×MIN_TILE_PAD here leaves
// exactly MIN_TILE_PAD of air after the divider (which is drawn MIN_TILE_PAD
// past the pinned edge — see scene()).
const DOCK_MIN_GAP: f32 = MIN_TILE_PAD * 1.5;
/// A minimized tile carries the window's aspect (wide window → wide tile),
/// clamped to this band so a pathological window can't grow a monstrous tile
/// or a hairline sliver. Aspect 1.0 lands exactly on the square dock slot.
const MIN_TILE_ASPECT: f32 = 0.4;
const MAX_TILE_ASPECT: f32 = 3.0;
/// A minimized tile is fit into a box at most this many times its full
/// height wide. A WIDE window (a wide-short terminal, aspect ~4) would
/// otherwise make a big wide tile at full icon height; capping the width
/// and letting the HEIGHT shrink instead keeps it a small tile (Max,
/// 2026-09-16: "it is a small window but the shape makes it bigger on the
/// dock… the window dont need to respect the height").
const MAX_TILE_W_FRAC: f32 = 1.7;

/// A minimized tile's drawn (width, height) for its window `aspect`, fit into
/// a `MAX_TILE_W_FRAC × full_h` box: full height for portrait/near-square
/// windows, width-capped with a shrunk height for wide ones. Used by both the
/// layout (slot width) and the draw so they always agree on the shape.
fn min_tile_dims(aspect: f32, full_h: f32) -> (f32, f32) {
    let a = aspect.clamp(MIN_TILE_ASPECT, MAX_TILE_ASPECT);
    let w_max = full_h * MAX_TILE_W_FRAC;
    if full_h * a > w_max {
        (w_max, w_max / a) // wide: cap the width, shrink the height
    } else {
        (full_h * a, full_h) // full height
    }
}
/// The thumbnail PICTURE is drawn smaller than a pinned dock icon (Max,
/// 2026-09-16: "make the thumbnails smaller") — this fraction of the dock
/// icon height, centered in the band. Its width follows the window aspect.
const MIN_TILE_SCALE: f32 = 0.90;
/// Horizontal air a minimized tile keeps beside its (smaller) picture, base
/// px scaled by `icon_scale`. Deliberately generous so the dock CARD grows
/// into a roomy shelf around the thumbnails (Max: "make the dock card grow")
/// even as the pictures shrink — it replaces the tiny `dock_slot − dock_icon`
/// breathing the tiles used to take.
const MIN_TILE_PAD: f32 = 8.0;
/// The macOS-style app-icon badge in a minimized tile's bottom-right corner:
/// its side as a fraction of the tile height, and its inset from that corner
/// (base px, scaled by `icon_scale`). Small — it identifies the app without
/// hiding the thumbnail.
const MIN_BADGE_FRAC: f32 = 0.45;
const MIN_BADGE_INSET: f32 = 2.0;
/// Peak scale of a grid icon under the cursor.
const GRID_MAGNIFY: f32 = 1.22;
/// Radial falloff radius of grid magnification, in pixels.
const GRID_MAG_RADIUS: f32 = 95.0;

/// Cosine ease from 1.0 at `d == 0` to 0.0 at `d >= radius`.
pub(crate) fn falloff(d: f32, radius: f32) -> f32 {
    let t = (d.abs() / radius).min(1.0);
    0.5 * (1.0 + (std::f32::consts::PI * t).cos())
}

/// Target magnification scale for one dock slot given the raw pointer —
/// the instantaneous value the smoothed `FrameInput::dock_mag` scales
/// ease toward each frame (the pointer steers, the easing draws).
pub(crate) fn dock_mag_target(
    slot: Rect,
    dock_hit_bottom: f32,
    pointer: Option<(f32, f32)>,
    icon_scale: f32,
    mag_amount: f32,
) -> f32 {
    let Some((px, py)) = pointer else {
        return 1.0;
    };
    let cx = slot.x + slot.w / 2.0;
    // Below the icon, the live column reaches `dock_hit_bottom` (the
    // screen edge while docked), so hovering the floating gap magnifies
    // the icon just like hovering it directly.
    let d_out = (slot.y - py).max(py - dock_hit_bottom).max(0.0);
    let fy = falloff(d_out, DOCK_MAG_VRADIUS);
    let dock_magnify = 1.0 + (DOCK_MAGNIFY - 1.0) * icon_scale;
    1.0 + (dock_magnify - 1.0) * falloff(px - cx, DOCK_MAG_RADIUS) * fy * mag_amount.clamp(0.0, 1.0)
}

/// [`dock_mag_target`] beside a side dock: the wave runs down the column,
/// live across the whole band (to the screen's edge while docked).
pub(crate) fn dock_mag_target_side(slot: Rect, band: Rect, pointer: Option<(f32, f32)>, icon_scale: f32, mag_amount: f32) -> f32 {
    let Some((px, py)) = pointer else {
        return 1.0;
    };
    let cy = slot.y + slot.h / 2.0;
    let d_out = (band.x - px).max(px - (band.x + band.w)).max(0.0);
    let fx = falloff(d_out, DOCK_MAG_VRADIUS);
    let dock_magnify = 1.0 + (DOCK_MAGNIFY - 1.0) * icon_scale;
    1.0 + (dock_magnify - 1.0) * falloff(py - cy, DOCK_MAG_RADIUS) * fx * mag_amount.clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A grid tile's name: an app's cut at its end (`truncate_label`), a
/// FILE's or a folder's — one with a path, not a web page's title — cut in
/// the middle with its ending kept (`desktop::fit_name`; Max, 2026-10-22),
/// by the same estimate.
pub(crate) fn tile_label(entry: &AppEntry, kind: Option<&crate::apps::EntryKind>, max_w: f32, font_px: f32) -> String {
    if kind != Some(&crate::apps::EntryKind::File) || entry.description.is_none() {
        return truncate_label(&entry.name, max_w, font_px);
    }
    let avg_char_w = font_px * 0.52;
    crate::desktop::fit_name(&entry.name, max_w + crate::desktop::FIT_SLACK, &mut |t| {
        t.chars().count() as f32 * avg_char_w
    })
}

/// Truncate a label to fit within `max_w` pixels, appending "…" when
/// the text is longer. Uses an average-character-width estimate (0.52×
/// font size) rather than shaping — close enough for app names.
pub(crate) fn truncate_label(text: &str, max_w: f32, font_px: f32) -> String {
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
    /// Title anchor: (centre, top) — the titles sit centred over their
    /// grids (Max, 2026-09-30).
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
    /// Cells per page for slot STORAGE (page-major slot math, `apps_cap`,
    /// `PagedList::normalize`) — derived from the screen alone (`fits`),
    /// never from how much content there happens to be. `rows` may shrink
    /// to the content span for display; capacity must not follow it: a
    /// transiently small span (daemon start, mid-index) fed a collapsed
    /// `rows` into `normalize`, which split the persisted pages at that
    /// tiny capacity and SAVED the shred (2026-09-03, "one line through
    /// lots of pages"). Under-full pages never re-merge, so a capacity
    /// collapse is permanent — this field is why it can't happen again.
    pub cap: usize,
    /// Search results: a row that isn't full splits about the title's
    /// centre like the Controls row (see [`SectionLayout::row_shift`]).
    /// Off for the resting grid, which keeps the order Max arranged.
    pub centre_rows: bool,
    /// THE LONE APP (`app_actions.rs`): the cell of the first row that
    /// stands under the title, the row laid out round it; `None` otherwise.
    pub anchor: Option<usize>,
    /// The pages go DOWN, not across (a side dock's box — Max, 2026-10-21):
    /// `scroll` is then along the height, a page `viewport.h` long.
    pub vertical: bool,
}

impl SectionLayout {
    /// A page's length along the way the pages go.
    pub fn page_len(&self) -> f32 {
        if self.vertical {
            self.viewport.h
        } else {
            self.viewport.w
        }
    }

    /// How far right the row holding display cell `i` is drawn: a row that
    /// isn't full, while searching, centres on the viewport's centre (the
    /// title's) — the middle result under the title for an odd count, the
    /// middle gap for an even one (Max, 2026-09-30). Full rows don't move.
    pub fn row_shift(&self, i: usize) -> f32 {
        let cols = self.cols.max(1);
        if !self.centre_rows {
            return 0.0;
        }
        let per_page = cols * self.rows.max(1);
        let start = i / per_page * per_page + (i % per_page) / cols * cols;
        if let Some(a) = self.anchor.filter(|_| start == 0) {
            return self.viewport.w / 2.0 - (a as f32 + 0.5) * self.viewport.w / cols as f32;
        }
        let k = self.cells.saturating_sub(start).min(cols);
        if k == 0 || k >= cols {
            return 0.0;
        }
        // Equal cells: the middle one (odd) or the middle gap (even) on
        // the centre is just the row centred.
        (self.viewport.w - k as f32 * self.viewport.w / cols as f32) / 2.0
    }
}

/// The dock's overflow state: the Apps tile's slot (the overflow tile), how
/// many pins are folded into it past the window's right end, and how far the
/// window can slide (in slots).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockOverflow {
    pub apps_slot: usize,
    pub hidden_right: usize,
    pub max_off: f32,
    /// Where the window of shown pins stands (0 = the first `cap` shown).
    pub off: f32,
    /// How many pins the window shows.
    pub cap: usize,
}

/// The QUICK BOX (Super+H, `panel.rs`): one setting in a box of its own
/// mid-screen — where, how present (0..1), and what the panel drew for it.
#[derive(Clone, Copy)]
pub struct QuickDraw<'a> {
    pub rect: Rect,
    pub a: f32,
    /// How big it is drawn, of `rect` and all that is on it, about the
    /// rect's centre: it comes and goes as a window does (`panel.rs`,
    /// "THE COMPOSITOR'S OWN WINDOW MOTION").
    pub scale: f32,
    pub draw: &'a crate::panel::PanelDraw,
    /// Its pictures: where each goes and the texture layer it is on.
    pub images: &'a [(Rect, u32, f32)],
    /// The names of its pictures that have no preview, on their tiles; and
    /// the ink they are written in.
    pub names: &'a [(Rect, String)],
    pub ink: [f32; 4],
    /// Its own colour this frame (the dock's, or — a wallpaper changing —
    /// already the new picture's: `App::quick_colours`).
    pub bg: [f32; 4],
}

impl std::fmt::Debug for QuickDraw<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QuickDraw {{ rect: {:?}, a: {} }}", self.rect, self.a)
    }
}

/// One of the quick controls' pills on the fast card (see
/// `App::fast_pills`): where, whether it is the one in front, its name as
/// what was typed and the rest with their shaped widths, its type size, and
/// how far out from behind the front one it is (0..1).
#[derive(Debug, Clone)]
pub struct FastPill {
    pub rect: Rect,
    pub front: bool,
    pub typed: String,
    pub rest: String,
    pub typed_w: f32,
    pub rest_w: f32,
    pub font_px: f32,
    pub show: f32,
}

/// A setting's pictures on their tiles. A thumbnail is the whole picture
/// centred on a clear square; the square is sized so the picture (of shape
/// `shape`, width over height) FILLS its tile, and it is cut to the tile's
/// rounded corners (`radius`). (Fitted inside, square-cornered, the tile's
/// ground and wash showed round it: Max, 2026-10-21, "the rectangles behind
/// the items show behind … weird corners".)
fn picture_tiles(scene: &mut Scene, images: &[(Rect, u32, f32)], radius: f32, chroma: Option<f32>) {
    picture_tiles_as(scene, images, radius, chroma, |r| r);
}

/// [`picture_tiles`], each tile put where `place` says (a box coming or
/// going: its pictures move with it).
/// (`chroma`: the palettes' cards under the sliders, `PanelDraw::card_chroma`.)
fn picture_tiles_as(scene: &mut Scene, images: &[(Rect, u32, f32)], radius: f32, chroma: Option<f32>, place: impl Fn(Rect) -> Rect) {
    // (A little past the tile all round: the picture's own outer texels
    // are half clear in its square, and must fall outside the cut.)
    let over = 1.0 + 4.0 / crate::apps::ICON_SIZE as f32;
    for &(tile, layer, shape) in images {
        let tile = place(tile);
        let shape = shape.clamp(0.2, 5.0);
        let side = over * if shape >= 1.0 { tile.w.max(tile.h * shape) } else { tile.h.max(tile.w / shape) };
        scene.overlay.push(IconInst {
            rect: Rect::new(tile.x + (tile.w - side) / 2.0, tile.y + (tile.h - side) / 2.0, side, side),
            layer,
            tint: [tile.x, tile.y, tile.x + tile.w, tile.y + tile.h],
            // A CLIPPED PICTURE (`icon.wgsl`): kept inside the tile's rounded rect.
            ring: -1.6,
            plate: [radius, 1.0, chroma.map_or(0.0, |c| 1.0 + c.max(0.0)), 0.0],
        });
    }
}

/// How solid what is ON the quick box is (its choices, its previews'
/// frames): the plate under each one's wash.
const QUICK_SOLID: f32 = 0.92;

/// What FAST LAUNCH draws this frame (see `fast_launch.rs`).
#[derive(Debug, Clone, Copy)]
pub struct FastDraw<'a> {
    /// The card's glass as it is now (reaching out for the sides), its
    /// corner radius, and its presence 0..1.
    pub card: Rect,
    pub radius: f32,
    pub a: f32,
    /// The bare icons in draw order (the front one last): entry, rect.
    pub icons: &'a [(usize, Rect)],
    /// QUICK CONTROLS: the settings as pills (the front one last); with
    /// them there are no icons and no word under.
    pub pills: &'a [FastPill],
    /// The word: what you typed (bright) then the rest of the name
    /// (dimmed), their shaped widths, the box it is centred in.
    pub typed: &'a str,
    pub rest: &'a str,
    pub typed_w: f32,
    pub rest_w: f32,
    pub text_box: Rect,
    pub font_px: f32,
    pub line_px: f32,
    /// The ink its words and pills wear: measured against what the clear
    /// card really shows (`App::fast_backdrop`), the dock's until then.
    pub ink: [f32; 4],
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
    /// The normal zone is uniform `DOCK_SLOT`-wide; each minimized-window
    /// tile (`>= dock_min_start`) carries its own aspect-derived width, so
    /// `.w` varies across the tail. `scene()` reads these widths back so the
    /// drawn thumbnail matches its hit-box.
    pub dock_slots: Vec<Rect>,
    /// First dock slot that is a minimized-window tile (`usize::MAX` when
    /// there are none). A small extra gap sits before this index, so the
    /// thumbnails read as their own group set off from the pinned icons
    /// (Max, 2026-09-16: "a little more space between the separator and the
    /// thumbnail"). `scene()` must apply the same gap to the drawn centers.
    pub dock_min_start: usize,
    /// That gap in scaled px (0 when no minimized tiles are shown).
    pub dock_min_gap: f32,
    /// Per dock slot, how much of it shows (1 = all): pins sliding out of
    /// the overflow window shrink toward 0 (see `layout`'s OVERFLOW).
    pub dock_slot_f: Vec<f32>,
    /// Set while the pins overflow into the Apps tile.
    pub dock_overflow: Option<DockOverflow>,
    /// The docked plate's resting width (the basin) — what the dock card
    /// spans while docked, grown to wrap the minimized tiles. The dock
    /// surface is full output width, so the input region is pinned to this
    /// centered width rather than the whole surface (see sync_input_region).
    pub dock_basin_w: f32,
    /// The popup sections, top to bottom (Apps, Install, Files).
    pub sections: [SectionLayout; N_SECTIONS],
    /// Fully expanded search box (centered, fixed width).
    pub search_box: Rect,
    /// Compact search button (circle, same center-x and y as search_box).
    pub search_btn: Rect,
    /// The rectangle the open box fills: its members in rows over its tile
    /// (a grid tile's cell, or a dock folder's icon). Present only while a
    /// box is open. A click inside is inert (only outside closes).
    pub open_box: Option<Rect>,
    /// That box's member grid: (columns, rows) of cells.
    pub open_box_grid: (usize, usize),
    /// How many members its line holds — its page (`row_cells`).
    pub open_box_cap: usize,
    /// The scale a stack over the dock draws its members at: the icon
    /// scale times its zoom (Ctrl +/−, `zoom.rs`).
    pub open_box_scale: f32,
    /// How much bigger than a label that stack's names are (`zoom.rs`).
    pub open_box_text: f32,
    /// The Apps rows parted around a box open in the grid.
    pub apps_part: Option<AppsPart>,
    /// The band under Files while a search matches settings or modules:
    /// each row's title position and the band its pills sit in (pill
    /// height, grid width) — the Controls row, then the Modules row.
    pub control_bands: Vec<((f32, f32), Rect)>,
    /// The control pills' rects in that band (filled in by the App, which
    /// measures their labels), in `SearchState::controls` order.
    pub controls: Vec<Rect>,
    /// The Apps rows' full room (every row that fits) and the room above
    /// the fixed sections: the App aims the trim with them.
    pub apps_full_h: f32,
    pub apps_avail: f32,
    /// How far the Controls band has opened (0..1): its row fades in with it.
    pub controls_k: f32,
    /// A search left no app: Apps is its title over a short "No results"
    /// line, its row going to Files ([`files_take_apps_row`]). Set by the App.
    pub apps_none: bool,
    /// The room the deep search's folded shelves gave the first
    /// ([`fold_shelves`]); 0 otherwise.
    pub shelf_room: f32,
    /// The deep search's tabs: their row (where the search field stood), how
    /// much smaller their words are drawn, and the page shown of how many
    /// (`App::layout_stretched`; their rects are `controls`).
    pub tabs_row: Option<Rect>,
    pub tabs_page: (usize, usize),
    /// Every tab where it stands when its page is shown, each page's size,
    /// and where the pages stand now (in pages: `Deep::slide`).
    pub tabs_at: Vec<Rect>,
    /// Each tab's word's width as drawn (its underline's).
    pub tabs_words: Vec<f32>,
    pub tabs_scales: Vec<f32>,
    pub tabs_slide: f32,
    /// The screen's edge the dock stands on (the Launcher page's Position):
    /// at the bottom a row the box rises from; on a side a column, the box
    /// growing out of that edge with the dock riding its far side
    /// ([`layout_side`]).
    pub side: Side,
    /// Where the dock's icons are live (hover, click, magnification): the
    /// dock's band, reaching the screen's edge while docked so the floating
    /// gap reacts too.
    pub dock_band: Rect,
}

impl Layout {
    /// Whether the dock is a column on a side of the screen.
    pub fn sideways(&self) -> bool {
        self.side != Side::Bottom
    }

    /// A slot's start and length along the dock (across under a bottom
    /// dock, down beside a side one).
    pub fn slot_span(&self, r: &Rect) -> (f32, f32) {
        if self.sideways() {
            (r.y, r.h)
        } else {
            (r.x, r.w)
        }
    }

    /// Whether `pos` is on the dock's live band.
    pub fn on_dock(&self, pos: (f32, f32)) -> bool {
        self.dock_band.contains(pos)
    }

    /// The box's room: the card less the dock's band (`band` its depth).
    pub fn box_area(&self, band: f32) -> Rect {
        match self.side {
            Side::Bottom => Rect::new(self.card_x, self.card_top + band, self.card_w, (self.card_h - band).max(0.0)),
            Side::Left => Rect::new(self.card_x, self.card_top, (self.card_w - band).max(0.0), self.card_h),
            Side::Right => Rect::new(self.card_x + band, self.card_top, (self.card_w - band).max(0.0), self.card_h),
        }
    }
}

/// Compute the layout for the current animation state.
///
/// `surface` is the full surface size, `extent` the card's rise,
/// `n_entries` the total (unfiltered) entry count for the dock, `n_min`
/// how many of those (the tail of `dock_order`) are minimized-window
/// tiles — the zone the dock widens to fit rather than clamp — `min_aspects`
/// those tiles' window aspects (dock-tail order, one per `n_min`) that shape
/// each tile's width, `n_visible` the filtered count per section, `scroll`
/// the unclamped per-section grid offsets.
// Geometry naturally takes many independent inputs; a params struct
// would just rename them.
/// The four icon-size levels, indexed by `App::icon_size` (default 1).
pub const ICON_SCALES: [f32; 4] = [0.70, 1.00, 1.30, 1.65];

/// Responsive fit: the largest fraction of the screen height the fully-open
/// card may occupy, and the floor the fit-cap may never push the scale below
/// (so text stays legible on a small panel). ICON_SCALES are absolute
/// multipliers of base sizes tuned for a large panel; without this cap the
/// upper levels make the open card taller than a short screen and clip its
/// top row off-display (seen on a 1366x768 laptop, 2026-09-02).
pub const OPEN_FIT: f32 = 0.94;
pub const OPEN_FIT_FLOOR: f32 = 0.6;

#[allow(clippy::too_many_arguments)]
pub fn layout(
    config: &Config,
    icon_scale: f32,
    surface: (f32, f32),
    extent: f32,
    n_entries: usize,
    n_min: usize,
    // The user's pins (the first `pins` of the dock order, before the fixed
    // trio) and how far their window is scrolled, in slots: once the row
    // would overflow the screen the pins slide through a window that ends
    // at the Apps tile — the overflow tile (Max, 2026-10-01).
    pins: usize,
    pin_off: f32,
    // Window aspects of the minimized tiles, in dock-tail order (`n_min`
    // long); each shapes its tile's width. Short/empty ⇒ square fallback.
    min_aspects: &[f32],
    n_visible: [usize; N_SECTIONS],
    scroll: [f32; N_SECTIONS],
    // Whether an app group ("box") is open over the Apps grid.
    box_open: bool,
    // AGUA deformation, (card silhouette, content ride) — 1.0 = rest.
    stretch: (f32, f32),
    // The Controls row's pill height while a search matches settings
    // (`None`: no row), and the room it has now — eased by the App toward
    // [`controls_band_h`] (0 without a row), so the sections make way
    // smoothly instead of jumping a row.
    controls_pill_h: Option<f32>,
    controls_band: f32,
    // How much shorter the open card is than its full height: the Apps
    // rows the page in view leaves empty (`App::apps_trim_target`, eased).
    // The card's bottom stays put; its top, the dock row and the Apps title
    // come down, and the Apps viewport shortens by as much.
    trim: f32,
    // How many pill rows the band holds: Controls, Modules, or both
    // (Max, 2026-10-03: "two different things, not mixed").
    control_rows: usize,
) -> Layout {
    // NOTE: icon_scale is already capped to fit the screen at its source,
    // App::icon_scale() (main.rs) — every extent and surface size derives from
    // that one value, so the cap lives there, not here. (2026-09-02)

    // Scaled size metrics — all icon/slot dimensions respond to icon_scale.
    let dock_slot = DOCK_SLOT * icon_scale;
    let dock_icon = DOCK_ICON * icon_scale;
    let dock_pad_x = DOCK_PAD_X * icon_scale;
    let grid_cell_w = GRID_CELL_W * icon_scale;
    let grid_cell_h = GRID_CELL_H * icon_scale;

    let (w, h) = surface;
    // The OPEN box width is fixed (the launcher grid is always this wide),
    // DECOUPLED from the surface: the dock surface is now full output width
    // so the dock plate can grow to fit minimized tiles, but the grid must
    // not balloon with it. This equals the old `w - 2·DRAG_MARGIN_X` back
    // when the surface hugged the box, so nothing about the open box moves.
    let card_w = config.window.width as f32 * icon_scale;
    // Dock slot budget. Nothing is clamped by the card width: the icons
    // WIDEN the dock instead, up to the whole screen. Normal (pinned/running)
    // icons used to stop at the open card's width, which silently dropped the
    // tail of the dock — the Recycle Bin first, as the dock filled up (Max,
    // 2026-09-29: "the dock should get as big as the full screen if needed").
    // Minimized-window tiles (`n_min`, the tail of `dock_order`) had already
    // lost that clamp for the same reason (Max, verified live: only ~2
    // thumbnails showed, the rest dropped): Max: "build them to the right; when they reach the edge,
    //     push the icons to the left; when there is no more room, the dock
    //     stretches." The row is centered on the surface, so honoring more
    //     tiles pushes the left content leftward on its own, and the basin
    //     (below) grows to wrap the wider row.
    //
    // The one ceiling is the physical surface: the whole row may not exceed
    // it less a dock-padding margin a side, or the rounded corners run off
    // the fixed-size surface. Past that hard cap the excess is held back — a
    // last resort that needs ~a monitor width of icons. Because
    // `dock_order` ends newest-last and the drawn row is its prefix, the very
    // newest minimize is the first held back; the count is logged on add.
    let n_normal = n_entries.saturating_sub(n_min);
    // A minimized tile is fit into a box (`min_tile_dims`): its width follows
    // the window aspect over a SMALLER picture height (`dock_icon ×
    // MIN_TILE_SCALE`), but a wide window is width-capped (its height shrinks)
    // so it never makes a big wide tile. Plus a generous `MIN_TILE_PAD` of
    // side air so the card grows into a roomy shelf. Variable-width tail.
    let min_tile_h = dock_icon * MIN_TILE_SCALE;
    let min_tile_pad = MIN_TILE_PAD * icon_scale;
    let min_slot_w = |k: usize| {
        let aspect = min_aspects.get(k).copied().unwrap_or(1.0);
        min_tile_dims(aspect, min_tile_h).0 + min_tile_pad
    };
    // OVERFLOW (Max, 2026-10-01): when the whole row won't fit the screen,
    // the pins give way, never the fixed trio or the running / minimized
    // zone. They show through a window of `pin_cap` slots ending at the Apps
    // tile (which becomes the overflow tile); `pin_off` slides it. An icon
    // leaving the window at either end shrinks away (`pin_f`), so the
    // window's width stays `pin_cap` slots throughout.
    let pins = pins.min(n_normal);
    let others_w = (n_normal - pins) as f32 * dock_slot
        + (0..n_min).map(min_slot_w).sum::<f32>()
        + if n_min > 0 { DOCK_MIN_GAP * icon_scale } else { 0.0 };
    let budget = w - 4.0 * dock_pad_x;
    let pin_room = ((budget - others_w) / dock_slot).floor();
    let pin_cap = (pin_room.max(1.0) as usize).min(pins.max(1));
    let overflow = pins > 0 && pin_room >= 1.0 && pins > pin_room as usize;
    let max_off = if overflow { (pins - pin_cap) as f32 } else { 0.0 };
    let off = pin_off.clamp(0.0, max_off);
    let pin_f = |i: usize| -> f32 {
        if !overflow || i >= pins {
            1.0
        } else {
            (i as f32 + 1.0 - off).min(off + pin_cap as f32 - i as f32).clamp(0.0, 1.0)
        }
    };
    let hard_cap = (((w - 2.0 * dock_pad_x) / dock_slot).floor() as usize).max(1);
    // Overflowing pins don't count against the cap: they fold into the tile.
    let n_normal_shown = if overflow { n_normal } else { n_normal.min(hard_cap) };
    // Minimized tiles earn their honored slots only once every normal icon is
    // shown: if the pins alone overflow the screen, the tail of
    // `dock_order` past `n_normal_shown` is a pinned icon, not a minimized
    // one, and the prefix slot→entry mapping must not shift under it.
    let n_min_shown = if overflow {
        n_min
    } else if n_normal_shown == n_normal {
        n_min.min(hard_cap.saturating_sub(n_normal_shown))
    } else {
        0
    };
    let n_dock = n_normal_shown + n_min_shown;
    // The minimized zone is set off by DOCK_MIN_GAP: the tiles begin at
    // `dock_min_start`, and everything from there is shifted right by the
    // gap (the row and card grow to hold it — see the basin below).
    let dock_min_start = if n_min_shown > 0 { n_normal_shown } else { usize::MAX };
    let dock_min_gap = if n_min_shown > 0 { DOCK_MIN_GAP * icon_scale } else { 0.0 };
    // Total base (unmagnified) width of the honored row: the uniform normal
    // slots, the variable minimized tail, and the group gap before the tail.
    let min_zone_w: f32 = (0..n_min_shown).map(min_slot_w).sum();
    let normal_w: f32 = (0..n_normal_shown).map(|i| dock_slot * pin_f(i)).sum();
    let row_base_w = normal_w + min_zone_w + dock_min_gap;
    let card_top = h - extent;
    let dock_h = config.window.input_bar_height as f32 * icon_scale;
    let float_gap = config.window.bottom_margin as f32 * icon_scale;
    // Hidden↔Dock: the bar keeps its full dock height and just slides up
    // out of the screen edge (clipped by it), lifting to float over the
    // last `float_gap` of travel — its shape never changes. Dock↔Open:
    // it grows upward from the pinned floating bottom.
    let card_h = (extent - float_gap).max(dock_h);
    // AGUA silhouette: at rest the water spreads wide in its basin;
    // rising gathers it to the box width; the live stretch conserves
    // volume — taller is narrower, and the landing squash spills wider.
    let full_extent = (config.window.height + config.window.bottom_margin) as f32 * icon_scale - trim;
    let dock_extent = dock_h + float_gap;
    let rise = ((extent - dock_extent) / (full_extent - dock_extent).max(1.0)).clamp(0.0, 1.0);
    // The dock stretches to wrap its row when its icons or the minimized
    // tiles push it past the default basin, never past the surface (a
    // dock-padding margin aside) and never below the default. The launcher
    // hides the tiles as it opens (main.rs gate); a row of pinned icons wider
    // than the box stays, and the open card keeps its width (`open_w`).
    // The RESTING dock width (its comfortable basin with no minimized tiles)
    // — the old `w - 2·BASIN_MARGIN_X` from when the surface hugged the box,
    // so the resting bar looks exactly as before. The card grows from here
    // up to the full surface as minimized tiles widen the row (Max,
    // 2026-09-16: "the dock plate should grow so more minimized apps fit").
    let resting_basin = card_w + 2.0 * (DRAG_MARGIN_X - BASIN_MARGIN_X);
    let max_basin = (w - 2.0 * dock_pad_x).max(resting_basin);
    let dock_row_w = row_base_w + 2.0 * dock_pad_x;
    let basin_w = dock_row_w.clamp(resting_basin, max_basin);
    // Risen, the card gathers to the box width — but never narrower than the
    // dock row it carries on top, or a full dock would hang off its sides.
    // Only the normal icons count: the minimized tiles hide as the card
    // opens, and the grid's columns (below) — hence the stored page
    // capacity — must not come and go with every minimized window.
    let open_row_w = normal_w + 2.0 * dock_pad_x;
    let open_w = card_w.max(open_row_w.min(max_basin));
    let gathered = basin_w + (open_w - basin_w) * rise;
    let card_w_now = (gathered * (1.0 - (stretch.0 - 1.0) * SPILL)).min(w);
    let card_x = (w - card_w_now) / 2.0;
    // Content is SHAPED at its fully-open geometry (`content_top` is
    // where the grid begins when open; `content_bottom` the pinned card
    // bottom) so rows never reflow mid-animation — then translated by
    // `y_off` so it rides the card: rising from the bottom edge as it
    // opens, sinking back into it as it closes (the reveal band clips
    // whatever sits past the card interior).
    let content_top = h - (config.window.height + config.window.bottom_margin) as f32 * icon_scale + trim;
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

    // `n_dock` (the honored slot count) and the widened basin were computed
    // up top so the card silhouette could grow with the row. The whole row
    // (`row_base_w`) centers on the surface, so it stays centered as the
    // variable tiles widen it — the same anchor `scene()` uses for the drawn
    // icon centers. Positions ACCUMULATE per-tile width (the tail varies),
    // and the min-gap opens once, at `dock_min_start`.
    let start_x = (w - row_base_w) / 2.0;
    let slot_y = card_top + (dock_h - dock_slot).max(0.0) / 2.0;
    let slot_h = dock_slot.min(dock_h);
    let mut dock_slots = Vec::with_capacity(n_dock);
    let mut slot_x = start_x;
    for i in 0..n_dock {
        if i == dock_min_start {
            slot_x += dock_min_gap;
        }
        let sw = if i >= dock_min_start {
            min_slot_w(i - dock_min_start)
        } else {
            dock_slot * pin_f(i)
        };
        dock_slots.push(Rect::new(slot_x, slot_y, sw, slot_h));
        slot_x += sw;
    }

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
    // The grid spreads to the sides with the card: a dock wider than the box
    // widens the card, and the sections gain the columns it made room for.
    let inner_w = (open_w - 2.0 * GRID_PAD_X).max(grid_cell_w);
    let cols = ((inner_w / grid_cell_w).floor() as usize).max(1);

    // Apps takes whatever rows fit after the fixed single-row sections
    // and the three title lines, capped at its design height (6×3 at
    // the default card size); short cards degrade to fewer rows.
    let fixed = (SECTION_ROWS[SECTION_INSTALL] + SECTION_ROWS[SECTION_FILES]) as f32 * grid_cell_h
        + N_SECTIONS as f32 * (SECTION_TITLE_H + DOT_ROOM)
        + (N_SECTIONS - 1) as f32 * SECTION_GAP;
    // Rows are counted on the untrimmed card: the trim only hides empty ones.
    let avail = grid_bottom - (grid_top - trim) - fixed;
    let fits = ((avail / grid_cell_h) as usize).clamp(1, SECTION_ROWS[SECTION_APPS]);
    // The Controls row (a search matching settings) takes its room out of
    // the Apps rows SHOWN — display only, like the shrink below: page
    // capacity stays at `fits`.
    // The pills sit as far under their title as the icons do under theirs,
    // and a little more (Max: "move only the pills a little lower").
    let controls_drop = GRID_ICON_TOP * icon_scale + CONTROLS_EXTRA_DROP;
    let control_rows = control_rows.max(1);
    let controls_h = controls_pill_h.map_or(0.0, |ph| controls_band_h(icon_scale, ph) * control_rows as f32);
    let fits_shown = (((avail - controls_h) / grid_cell_h) as usize).clamp(1, fits);
    // Apps shows every row that fits; a page that doesn't fill them
    // shortens the card instead (`trim`, Max 2026-09-30: "card shrinks to
    // fit"). That replaced the old display shrink, which parked the unused
    // rows as air above Search. Page capacity stays at `fits` either way
    // (`SectionLayout::cap`): the stored order never re-pages.
    let apps_rows = fits_shown;
    // The Apps viewport's height, continuous: every row that fits, less the
    // room the Controls band has now (from the spare air first), less the
    // trim. The App aims the trim so it shows exactly the rows it needs.
    let apps_room = (fits as f32 * grid_cell_h).min(avail - controls_band);
    let apps_h = (apps_room - trim).max(0.0);
    let apps_cap_rows = fits;

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
        let title_pos = (grid_x0 + cols as f32 * grid_cell_w / 2.0, agua(y + y_off));
        y += SECTION_TITLE_H;
        let viewport = Rect::new(
            grid_x0,
            agua(y + y_off),
            cols as f32 * grid_cell_w,
            if s == SECTION_APPS { apps_h } else { rows as f32 * grid_cell_h },
        );
        // (The Apps' room as trimmed, even past nothing — drawn at nothing —:
        // what follows stays where the card's bottom holds it. The deep
        // search's folded shelves give the first theirs back, `fold_shelves`.)
        y += if s == SECTION_APPS { apps_room - trim } else { viewport.h } + DOT_ROOM + SECTION_GAP;
        // Horizontal paging: each page is viewport.w wide. Pages wrap
        // cyclically (infinite scroll), so the offset is normalized into
        // [0, n_pages * viewport.w) rather than clamped.
        let cells_per_page = cols * rows;
        let n_pages = if n_visible[s] == 0 {
            0
        } else {
            n_visible[s].div_ceil(cells_per_page.max(1)).max(1)
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
            cap: cols
                * if s == SECTION_APPS {
                    apps_cap_rows
                } else {
                    rows
                },
            centre_rows: false,
            anchor: None,
            vertical: false,
        }
    });

    // A box open shows a square as tall as the whole Apps grid (top to
    // bottom edges), centered horizontally — its rest rect, which the
    // magnified box grows into and which swallows clicks.
    let open_box = box_open.then(|| {
        let vp = sections[SECTION_APPS].viewport;
        // Size the box at exactly 3 scaled cells (OPEN_BOX_COLS rows/cols)
        // so icons inside track icon_scale directly. Cap at the viewport.
        let side = (OPEN_BOX_COLS as f32 * grid_cell_h).min(vp.h).min(vp.w);
        Rect::new(vp.x + (vp.w - side) / 2.0, vp.y, side, side)
    });

    // The Controls row: titled like a section, its pills in a band under
    // the title.
    let files_bottom = sections[SECTION_FILES].viewport.y + sections[SECTION_FILES].viewport.h;
    // One band per row, each titled like a section.
    let control_bands: Vec<((f32, f32), Rect)> = match controls_pill_h {
        Some(ph) => {
            let vp = sections[SECTION_FILES].viewport;
            let band = controls_band_h(icon_scale, ph);
            (0..control_rows)
                .map(|r| {
                    let title_y = files_bottom + DOT_ROOM + SECTION_GAP + r as f32 * band;
                    ((vp.x + vp.w / 2.0, title_y), Rect::new(vp.x, title_y + SECTION_TITLE_H + controls_drop, vp.w, ph))
                })
                .collect()
        }
        None => Vec::new(),
    };
    // Under the last section's dots.
    let above_search = control_bands.last().map_or(files_bottom, |b| b.1.y + b.1.h) + DOT_ROOM + LAST_SECTION_AIR;
    // Lift the search pill into whatever room the sections left above it.
    let lift = (SEARCH_LIFT * icon_scale).min((search_box.y - above_search - SEARCH_LIFT_AIR).max(0.0));
    let search_box = Rect::new(search_box.x, search_box.y - lift, search_box.w, search_box.h);
    let search_btn = Rect::new(search_btn.x, search_btn.y - lift, search_btn.w, search_btn.h);

    Layout {
        card_x,
        card_w: card_w_now,
        card_top,
        card_h,
        dock_hit_bottom,
        dock_slots,
        dock_min_start,
        dock_min_gap,
        dock_slot_f: (0..n_dock).map(pin_f).collect(),
        dock_overflow: overflow.then(|| DockOverflow {
            apps_slot: pins,
            hidden_right: (0..pins).filter(|&i| i as f32 >= off && pin_f(i) < 0.5).count(),
            max_off,
            off,
            cap: pin_cap,
        }),
        dock_basin_w: basin_w,
        sections,
        search_box,
        search_btn,
        open_box,
        open_box_grid: (OPEN_BOX_COLS, OPEN_BOX_COLS),
        open_box_cap: OPEN_BOX_CAP,
        open_box_scale: icon_scale,
        open_box_text: 1.0,
        apps_part: None,
        control_bands,
        controls: Vec::new(),
        apps_full_h: fits as f32 * grid_cell_h,
        apps_avail: avail,
        controls_k: if controls_h > 0.0 { (controls_band / controls_h).clamp(0.0, 1.0) } else { 0.0 },
        apps_none: false,
        shelf_room: 0.0,
        tabs_row: None,
        tabs_page: (0, 1),
        tabs_at: Vec::new(),
        tabs_words: Vec::new(),
        tabs_scales: Vec::new(),
        tabs_slide: 0.0,
        side: Side::Bottom,
        dock_band: Rect::new(0.0, card_top, w, (dock_hit_bottom - card_top).max(0.0)),
    }
}

/// THE BOX ON ITS SIDE (Max, 2026-10-21, the lab
/// https://claude.ai/artifact/CxrYJqk12ekLbZTDzyz5mw: "it is gonna be the same
/// all the same"): the Apps grid three across, every section six down —
/// Apps keeps its 18 to a page, Install and Files their 6 — paged downwards.
const SIDE_APPS_COLS: usize = 3;
const SIDE_ROWS: usize = 6;
/// The resting side dock hugs its icons: this much air past the dock's own
/// padding at each end (a column as long as the bottom dock's basin is wide
/// would run the screen's height).
const SIDE_BASIN_AIR: f32 = 14.0;
/// From the box's top to the titles' line, and under the search pill.
const SIDE_TOP: f32 = GRID_TOP_GAP + 5.0;
const SIDE_BOTTOM: f32 = 12.0;
/// The grid's cells are narrower beside a side dock: as wide as they are
/// tall, the icons closer across (Max, 2026-10-21: "the horizontal gaps
/// between the icons should be less — but not the gap between sections").
pub const SIDE_CELL_W: f32 = GRID_CELL_H;
/// How far past its section's ICONS a side box's page dots stand (their
/// centres): right by them (Max, 2026-10-21: "closer to their icons").
const SIDE_DOTS_FROM_ICONS: f32 = 6.0;

/// How wide the open box is beside a side dock: the dock's band, the
/// padding, and Files · Install · Apps with the gaps between them (each
/// section's page dots stand in the gap after it, the last one's in the
/// padding).
pub fn side_open_w(config: &Config, icon_scale: f32) -> f32 {
    let band = config.window.input_bar_height as f32 * icon_scale;
    let cell = SIDE_CELL_W * icon_scale;
    band + 2.0 * GRID_PAD_X + (2 + SIDE_APPS_COLS) as f32 * cell + 2.0 * SECTION_GAP
}

/// How tall the Apps' line is when a search leaves no app: its "No
/// results", under the title.
pub const NO_APPS_H: f32 = 30.0;

/// A SEARCH THAT LEFT NO APP (Max, 2026-10-27: "when there is no app matched,
/// we should show the apps empty, install (1 line) and two lines of files"):
/// Apps is its title over a short "No results" line ([`NO_APPS_H`], what the
/// App's trim aims at; `Layout::apps_none`), Install keeps its line, and Files
/// take the row Apps gave up as a SECOND line when they fill more than one.
/// `extra` is how much of that row they have now — eased by the App, so
/// Install and Files glide up as the line opens and back as it closes (cells
/// are as tall as they are wide, whatever the viewport: a part-open second
/// line is only clipped). It is taken out of the Apps' room, so Files'
/// bottom (the System row, the search) stays put. `scroll` is Files' pager
/// position as it is, wrapped again for the two-line pages.
pub fn files_take_apps_row(l: &mut Layout, extra: f32, scroll: f32) {
    let extra = extra.min(l.sections[SECTION_APPS].viewport.h);
    if extra <= 0.0 || l.sections[SECTION_FILES].vertical {
        return;
    }
    l.sections[SECTION_APPS].viewport.h -= extra;
    for s in [SECTION_INSTALL, SECTION_FILES] {
        l.sections[s].title_pos.1 -= extra;
        l.sections[s].viewport.y -= extra;
    }
    let f = &mut l.sections[SECTION_FILES];
    f.viewport.h += extra;
    f.rows += 1;
    f.cap = f.cols * f.rows;
    f.n_pages = if f.cells == 0 { 0 } else { f.cells.div_ceil(f.cap).max(1) };
    let total_w = f.n_pages as f32 * f.viewport.w;
    f.scroll = if total_w > 0.0 { scroll.rem_euclid(total_w) } else { 0.0 };
}

/// THE DEEP SEARCH'S SHELVES (`deep_search.rs`): the sections, a shelf with
/// nothing on it FOLDED away — no title, no row — and its room the first
/// shelf's, which shows as many more rows as that holds (its pages hold them
/// too). What is under the shelves stays put; the card's trim shortens the
/// first to what its page needs (`App::apps_trim_target`). `scroll` is the
/// first's pager position as it is, wrapped again for its bigger pages.
pub fn fold_shelves(l: &mut Layout, hidden: [bool; N_SECTIONS], cell_h: f32, scroll: f32) {
    if l.sections[SECTION_APPS].vertical {
        return;
    }
    // Each hidden one's room: from its title to the next's (the last's: past
    // its dots and the gap after them).
    let span = |l: &Layout, s: usize| -> f32 {
        let end = match l.sections.get(s + 1) {
            Some(next) => next.title_pos.1,
            None => l.sections[s].viewport.y + l.sections[s].viewport.h + DOT_ROOM + SECTION_GAP,
        };
        end - l.sections[s].title_pos.1
    };
    let rooms: [f32; N_SECTIONS] = std::array::from_fn(|s| if s > 0 && hidden[s] { span(l, s) } else { 0.0 });
    let total: f32 = rooms.iter().sum();
    if total <= 0.0 {
        return;
    }
    // The first's room as trimmed (the layout draws it at nothing at least),
    // read before anything moves; it takes the hidden shelves' too.
    let room = l.sections[SECTION_INSTALL].title_pos.1 - l.sections[SECTION_APPS].viewport.y - DOT_ROOM - SECTION_GAP;
    for s in 1..N_SECTIONS {
        if hidden[s] {
            let sec = &mut l.sections[s];
            sec.viewport.h = 0.0;
            sec.cells = 0;
            sec.n_pages = 0;
            continue;
        }
        let after: f32 = rooms[s + 1..].iter().sum();
        l.sections[s].title_pos.1 += after;
        l.sections[s].viewport.y += after;
    }
    let first = &mut l.sections[SECTION_APPS];
    first.viewport.h = (room + total).max(0.0);
    first.rows += (total / cell_h).floor() as usize;
    first.cap = first.cols * first.rows;
    first.n_pages = if first.cells == 0 { 0 } else { first.cells.div_ceil(first.cap.max(1)).max(1) };
    let total_w = first.n_pages as f32 * first.viewport.w;
    first.scroll = if total_w > 0.0 { scroll.rem_euclid(total_w) } else { 0.0 };
    l.shelf_room = total;
}

/// [`layout`] for the dock on `side`: at the bottom as it always was, on
/// the left or right turned on its side ([`layout_side`]).
#[allow(clippy::too_many_arguments)]
pub fn layout_on(
    side: Side,
    config: &Config,
    icon_scale: f32,
    surface: (f32, f32),
    extent: f32,
    n_entries: usize,
    n_min: usize,
    pins: usize,
    pin_off: f32,
    min_aspects: &[f32],
    n_visible: [usize; N_SECTIONS],
    scroll: [f32; N_SECTIONS],
    box_open: bool,
    stretch: (f32, f32),
    controls_pill_h: Option<f32>,
    controls_band: f32,
    trim: f32,
    control_rows: usize,
    // How much of the screen's top the banner takes: a side dock stops
    // under it.
    top_reserved: f32,
) -> Layout {
    if side == Side::Bottom {
        return layout(
            config, icon_scale, surface, extent, n_entries, n_min, pins, pin_off, min_aspects, n_visible, scroll, box_open, stretch, controls_pill_h, controls_band, trim,
            control_rows,
        );
    }
    layout_side(
        config,
        icon_scale,
        surface,
        extent,
        n_entries,
        n_min,
        pins,
        pin_off,
        n_visible,
        scroll,
        box_open,
        controls_pill_h,
        controls_band,
        control_rows,
        top_reserved,
        side == Side::Left,
    )
}

/// [`layout`] for a dock on the LEFT or RIGHT edge. The same box turned on
/// its side: the dock a column, centred on the screen's height; the card's
/// edge side pinned a gap off the screen's edge, reaching `extent` in —
/// resting it hugs the dock, opening it gathers to the box's height about
/// the screen's middle, and the dock rides its FAR side (as at the bottom it
/// rides the box's top). From the edge to the dock: Files, Install, Apps,
/// their titles on one line at the top, the search pill at the bottom.
#[allow(clippy::too_many_arguments)]
fn layout_side(
    config: &Config,
    icon_scale: f32,
    surface: (f32, f32),
    extent: f32,
    n_entries: usize,
    n_min: usize,
    pins: usize,
    pin_off: f32,
    n_visible: [usize; N_SECTIONS],
    scroll: [f32; N_SECTIONS],
    box_open: bool,
    controls_pill_h: Option<f32>,
    controls_band: f32,
    control_rows: usize,
    top_reserved: f32,
    left: bool,
) -> Layout {
    let (w, h) = surface;
    let dock_slot = DOCK_SLOT * icon_scale;
    let dock_pad = DOCK_PAD_X * icon_scale;
    let cell_w = SIDE_CELL_W * icon_scale;
    let cell_h = GRID_CELL_H * icon_scale;
    let band = config.window.input_bar_height as f32 * icon_scale;
    let gap = config.window.bottom_margin as f32 * icon_scale;
    let open_w = side_open_w(config, icon_scale);
    // The dock's whole run (Max, 2026-10-21: "all the way to the bottom and
    // the top, not over the banner"): from a gap under the banner to a gap
    // off the screen's bottom; the column and the box centre on it.
    let span_top = top_reserved + gap;
    let span = (h - gap - span_top).max(band);
    let mid = span_top + span / 2.0;

    // The column: square slots (a minimized window's picture fits its
    // square). A column longer than the screen holds OVERFLOWS as the
    // bottom row does (`layout`): the pins give way, sliding through a
    // window that ends at the Apps button, never the trio, the running or
    // the minimized ones — only once the column runs the whole span, so the
    // Apps button eats icons only when there is no room left.
    let n_normal = n_entries.saturating_sub(n_min);
    let pins = pins.min(n_normal);
    let others = (n_normal - pins + n_min) as f32 * dock_slot + if n_min > 0 { DOCK_MIN_GAP * icon_scale } else { 0.0 };
    let budget = span - 2.0 * dock_pad;
    let pin_room = ((budget - others) / dock_slot).floor();
    let pin_cap = (pin_room.max(1.0) as usize).min(pins.max(1));
    let overflow = pins > 0 && pin_room >= 1.0 && pins > pin_room as usize;
    let max_off = if overflow { (pins - pin_cap) as f32 } else { 0.0 };
    let off = pin_off.clamp(0.0, max_off);
    let pin_f = |i: usize| -> f32 {
        if !overflow || i >= pins {
            1.0
        } else {
            (i as f32 + 1.0 - off).min(off + pin_cap as f32 - i as f32).clamp(0.0, 1.0)
        }
    };
    let hard_cap = (((span - 2.0 * dock_pad) / dock_slot).floor() as usize).max(1);
    let n_normal_shown = if overflow { n_normal } else { n_normal.min(hard_cap) };
    let n_min_shown = if overflow {
        n_min
    } else if n_normal_shown == n_normal {
        n_min.min(hard_cap - n_normal_shown)
    } else {
        0
    };
    let n_dock = n_normal_shown + n_min_shown;
    let dock_min_start = if n_min_shown > 0 { n_normal_shown } else { usize::MAX };
    let dock_min_gap = if n_min_shown > 0 { DOCK_MIN_GAP * icon_scale } else { 0.0 };
    let slot_len = |i: usize| if i < n_normal_shown { dock_slot * pin_f(i) } else { dock_slot };
    let row_len = (0..n_dock).map(slot_len).sum::<f32>() + dock_min_gap;
    let air = 2.0 * (dock_pad + SIDE_BASIN_AIR * icon_scale);
    let basin_h = (row_len + air).min(span);
    // The plate the box must hold when open: the normal icons only (the
    // minimized ones hide as it opens), as at the bottom.
    let open_plate = ((0..n_normal_shown).map(slot_len).sum::<f32>() + air).min(span);

    // The open box's rows: six, or as many as a longer dock makes room for
    // (Max, 2026-10-21: "the box should adapt to the size of the dock" — as
    // the bottom box widens with a long dock and its sections gain
    // columns, here the sections gain ROWS); never more than the span
    // holds. A search matching settings takes its band out of the rows shown.
    let controls_drop = GRID_ICON_TOP * icon_scale + CONTROLS_EXTRA_DROP;
    let control_rows = control_rows.max(1);
    let controls_h = controls_pill_h.map_or(0.0, |ph| controls_band_h(icon_scale, ph) * control_rows as f32);
    let fixed = SIDE_TOP + SECTION_TITLE_H + LAST_SECTION_AIR + SEARCH_GAP + SEARCH_H + SIDE_BOTTOM;
    let room = span - fixed;
    let wanted = ((open_plate - fixed) / cell_h).floor().max(SIDE_ROWS as f32) as usize;
    let fits = ((room / cell_h).max(1.0) as usize).clamp(1, wanted);
    let rows = (((room - controls_h) / cell_h).max(1.0) as usize).clamp(1, fits);
    // As tall as its rows, and never shorter than the dock it carries.
    let open_h = (fixed + rows as f32 * cell_h + controls_band).max(open_plate).min(span);

    // The card.
    let dock_extent = band + gap;
    let full_extent = open_w + gap;
    let rise = ((extent - dock_extent) / (full_extent - dock_extent).max(1.0)).clamp(0.0, 1.0);
    let card_w = (extent - gap).max(band);
    let card_x = if left { extent - card_w } else { w - extent };
    let card_h = basin_h + (open_h - basin_h) * rise;
    let card_top = mid - card_h / 2.0;

    // The dock's band, on the card's far side.
    let band_x = if left { card_x + card_w - band } else { card_x };
    let slot_x = band_x + (band - dock_slot) / 2.0;
    let mut dock_slots = Vec::with_capacity(n_dock);
    let mut y = mid - row_len / 2.0;
    for i in 0..n_dock {
        if i == dock_min_start {
            y += dock_min_gap;
        }
        dock_slots.push(Rect::new(slot_x, y, dock_slot, slot_len(i)));
        y += slot_len(i);
    }
    let docked = card_w <= band + 0.5;
    let dock_band = match (docked, left) {
        (true, true) => Rect::new(0.0, card_top, card_x + card_w, card_h),
        (true, false) => Rect::new(card_x, card_top, w - card_x, card_h),
        (false, _) => Rect::new(band_x, card_top, band, card_h),
    };

    // The sections stand where the open box has them and ride the card out
    // of the edge (trailing it a touch, as at the bottom).
    let full_x = if left { gap } else { w - gap - open_w };
    let ride = if left {
        -((full_x + open_w) - (card_x + card_w)).max(0.0) * RIDE_PARALLAX
    } else {
        (card_x - full_x).max(0.0) * RIDE_PARALLAX
    };
    let open_top = mid - open_h / 2.0;
    let title_y = open_top + SIDE_TOP;
    let vp_y = title_y + SECTION_TITLE_H;
    let cols_of = |s: usize| if s == SECTION_APPS { SIDE_APPS_COLS } else { 1 };
    // From the screen's edge to the dock: Files, Install, Apps (Max).
    let order: [usize; N_SECTIONS] = if left {
        [SECTION_FILES, SECTION_INSTALL, SECTION_APPS]
    } else {
        [SECTION_APPS, SECTION_INSTALL, SECTION_FILES]
    };
    let mut xs = [0.0f32; N_SECTIONS];
    let mut x = if left { full_x + GRID_PAD_X } else { full_x + band + GRID_PAD_X };
    for &s in &order {
        xs[s] = x + ride;
        x += cols_of(s) as f32 * cell_w + SECTION_GAP;
    }
    let sections: [SectionLayout; N_SECTIONS] = std::array::from_fn(|s| {
        let cols = cols_of(s);
        let viewport = Rect::new(xs[s], vp_y, cols as f32 * cell_w, rows as f32 * cell_h);
        let n_pages = if n_visible[s] == 0 { 0 } else { n_visible[s].div_ceil((cols * rows).max(1)).max(1) };
        let total = n_pages as f32 * viewport.h;
        SectionLayout {
            title_pos: (viewport.x + viewport.w / 2.0, title_y),
            viewport,
            cols,
            rows,
            n_pages,
            scroll: if total > 0.0 { scroll[s].rem_euclid(total) } else { 0.0 },
            cells: n_visible[s],
            cap: cols * fits,
            centre_rows: false,
            anchor: None,
            vertical: true,
        }
    });
    let first = &sections[order[0]].viewport;
    let last = &sections[order[N_SECTIONS - 1]].viewport;
    let cx = (first.x + last.x + last.w) / 2.0;
    let vp_bottom = vp_y + rows as f32 * cell_h;

    // A search matching settings: their row(s) under the sections, as
    // wide as the three.
    let control_bands: Vec<((f32, f32), Rect)> = match controls_pill_h {
        Some(ph) => {
            let bh = controls_band_h(icon_scale, ph);
            (0..control_rows)
                .map(|r| {
                    let ty = vp_bottom + DOT_ROOM + SECTION_GAP + r as f32 * bh;
                    ((cx, ty), Rect::new(first.x, ty + SECTION_TITLE_H + controls_drop, last.x + last.w - first.x, ph))
                })
                .collect()
        }
        None => Vec::new(),
    };
    let search_y = open_top + open_h - SIDE_BOTTOM - SEARCH_H;
    let search_box = Rect::new(cx - SEARCH_W_MIN / 2.0, search_y, SEARCH_W_MIN, SEARCH_H);
    let search_btn = Rect::new(cx - SEARCH_BTN_W / 2.0, search_y, SEARCH_BTN_W, SEARCH_H);
    let open_box = box_open.then(|| {
        let vp = sections[SECTION_APPS].viewport;
        let side = (OPEN_BOX_COLS as f32 * cell_h).min(vp.h).min(vp.w);
        Rect::new(vp.x + (vp.w - side) / 2.0, vp.y, side, side)
    });

    Layout {
        card_x,
        card_w,
        card_top,
        card_h,
        dock_hit_bottom: card_top + card_h,
        dock_slots,
        dock_min_start,
        dock_min_gap,
        dock_slot_f: (0..n_dock).map(pin_f).collect(),
        dock_overflow: overflow.then(|| DockOverflow {
            apps_slot: pins,
            hidden_right: (0..pins).filter(|&i| i as f32 >= off && pin_f(i) < 0.5).count(),
            max_off,
            off,
            cap: pin_cap,
        }),
        // (Along a side the plate's resting LENGTH: its height.)
        dock_basin_w: basin_h,
        sections,
        search_box,
        search_btn,
        open_box,
        open_box_grid: (OPEN_BOX_COLS, OPEN_BOX_COLS),
        open_box_cap: OPEN_BOX_CAP,
        open_box_scale: icon_scale,
        open_box_text: 1.0,
        apps_part: None,
        control_bands,
        controls: Vec::new(),
        apps_full_h: fits as f32 * cell_h,
        apps_avail: room,
        controls_k: if controls_h > 0.0 { (controls_band / controls_h).clamp(0.0, 1.0) } else { 0.0 },
        apps_none: false,
        shelf_room: 0.0,
        tabs_row: None,
        tabs_page: (0, 1),
        tabs_at: Vec::new(),
        tabs_words: Vec::new(),
        tabs_scales: Vec::new(),
        tabs_slide: 0.0,
        side: if left { Side::Left } else { Side::Right },
        dock_band,
    }
}

/// Return the horizontal scroll offset (page * viewport.w) that makes
/// `cell` visible in its section, snapping to the page containing it.
pub fn scroll_to_reveal(section: &SectionLayout, cell: usize) -> f32 {
    let cells_per_page = (section.cols * section.rows).max(1);
    let page = cell / cells_per_page;
    let max_page = section.n_pages.saturating_sub(1);
    (page.min(max_page) as f32 * section.page_len()).max(0.0)
}

/// The room a Controls row takes under Files: the gap, its title, the drop
/// to the pills, the pills and their dots' room.
pub fn controls_band_h(icon_scale: f32, pill_h: f32) -> f32 {
    SECTION_GAP + SECTION_TITLE_H + GRID_ICON_TOP * icon_scale + CONTROLS_EXTRA_DROP + pill_h + DOT_ROOM
}

/// Lay the Controls row's pills (`widths`, best match first) in `row`
/// about its centre — the title's (Max, 2026-09-30): an odd count puts the
/// middle pill on it, an even count the gap between the middle two. Only
/// as many as fit inside the row are shown, best first.
/// Which of `widths` (in their order) `row` holds as [`controls_rects`]
/// lays them out: each in turn if it still fits beside those already taken
/// — one that does not is passed over and the next ones keep their chance.
pub(crate) fn row_fits(row: Rect, widths: &[f32], gap: f32) -> Vec<usize> {
    let mut kept: Vec<usize> = Vec::new();
    for i in 0..widths.len() {
        let mut with: Vec<f32> = kept.iter().map(|&k| widths[k]).collect();
        with.push(widths[i]);
        if controls_rects(row, &with, gap).len() == with.len() {
            kept.push(i);
        }
    }
    kept
}

pub(crate) fn controls_rects(row: Rect, widths: &[f32], gap: f32) -> Vec<Rect> {
    let cx = row.x + row.w / 2.0;
    for n in (1..=widths.len()).rev() {
        let ws = &widths[..n];
        // Where the first pill starts, relative to the centre.
        let before: f32 = ws[..n / 2].iter().map(|w| w + gap).sum();
        let start = if n % 2 == 1 {
            cx - ws[n / 2] / 2.0 - before
        } else {
            cx - before + gap / 2.0
        };
        let end = start + ws.iter().sum::<f32>() + gap * (n - 1) as f32;
        if start < row.x - 0.01 || end > row.x + row.w + 0.01 {
            continue;
        }
        let mut x = start;
        return ws
            .iter()
            .map(|&w| {
                let r = Rect::new(x, row.y, w, row.h);
                x += w + gap;
                r
            })
            .collect();
    }
    Vec::new()
}

/// Which section's scroll band contains `pos`: the viewport plus its
/// title line above and the gap (page dots) beneath, so wheel paging is
/// forgiving about the exact pointer height.
pub fn section_at(layout: &Layout, pos: (f32, f32)) -> Option<usize> {
    // Side by side (a side dock): the column, its title over it and half
    // the gap on each side.
    if layout.sideways() {
        return layout.sections.iter().position(|sec| {
            pos.0 >= sec.viewport.x - SECTION_GAP / 2.0
                && pos.0 < sec.viewport.x + sec.viewport.w + SECTION_GAP / 2.0
                && pos.1 >= sec.viewport.y - SECTION_TITLE_H - TITLE_LINE_PX
                && pos.1 < sec.viewport.y + sec.viewport.h + DOT_ROOM
        });
    }
    layout.sections.iter().position(|sec| {
        pos.1 >= sec.viewport.y - SECTION_TITLE_H
            && pos.1 < sec.viewport.y + sec.viewport.h + DOT_ROOM + SECTION_GAP
    })
}

/// A dock stack's members (Max, 2026-10-13: "from the dock I want the names
/// under the icons… a little bigger and apart"): the dock's icons a size
/// up, in cells wide enough for a name under each.
pub const STACK_ICON: f32 = 46.0;
pub const STACK_CELL_W: f32 = 78.0;
const STACK_ICON_TOP: f32 = 8.0;
const STACK_BOTTOM_PAD: f32 = 6.0;

/// A dock stack's cell height at `icon_scale`: the icon, its name (at
/// `text` times the label's size — the name follows the stack's zoom at
/// its own, slower pace) and the air around them.
pub fn stack_cell_h(icon_scale: f32, text: f32) -> f32 {
    (STACK_ICON_TOP + STACK_ICON + STACK_BOTTOM_PAD) * icon_scale + GRID_LABEL_GAP + LABEL_LINE_PX * text
}

/// Gap between a dock folder's open box and its icon: the page dots sit
/// in it.
pub const BOX_GAP: f32 = 12.0;

/// The Apps grid parted around a box open in it (Max, 2026-10-13: "como
/// macOS, la línea se abre abajo del icono, y la línea de abajo se mueve"):
/// the box's row opens under its tile's row and the rows below make room.
/// Where the grid can't grow any further the tile's row and those over it
/// give the rest, moving up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppsPart {
    /// The tile's row; -1 for a box from the dock (every row is below it).
    pub row: isize,
    /// How far rows up to `row` have moved up, and the rest down (px, at
    /// the open animation's progress).
    pub up: f32,
    pub down: f32,
}

impl AppsPart {
    /// How far grid row `row` is from its resting place.
    pub fn shift(&self, row: usize) -> f32 {
        if row as isize <= self.row {
            -self.up
        } else {
            self.down
        }
    }

    /// The resting height of what is drawn at `y`, in a grid whose rows
    /// are `cell_h` high from `top`; `None` in the room the rows made.
    pub fn rest_y(&self, top: f32, cell_h: f32, y: f32) -> Option<f32> {
        let split = top + (self.row + 1) as f32 * cell_h;
        if y < split - self.up {
            Some(y + self.up)
        } else if y >= split + self.down {
            Some(y - self.down)
        } else {
            None
        }
    }
}

/// An open box's line: a box of `n` members in `cell`s across `area_w`.
/// The line holds as many as fit (Max, 2026-10-13: "tantos íconos como
/// entren en la línea antes de que sea necesaria una página"); only what
/// is over that goes to a next page. The cells are those of where it is
/// open (Max, same day: "que respete la estética del dock… de la caja"):
/// the dock's slots over the dock, the grid's cells in the grid. Returns
/// its size, its (columns, rows) and how many members a page of it holds.
pub fn row_cells(n: usize, cell: (f32, f32), area_w: f32) -> ((f32, f32), (usize, usize), usize) {
    let cell_w = cell.0.max(1.0);
    let cap = ((area_w / cell_w + 0.001).floor() as usize).max(1);
    let cols = n.clamp(1, cap);
    ((cols as f32 * cell_w, cell.1), (cols, 1), cap)
}

/// A dock folder's open box at rest: its line (`row_cells`) centred over
/// `anchor` (the dock icon) and `gap` above it, kept inside `area`.
pub fn row_box(
    anchor: Rect,
    n: usize,
    cells: (f32, f32),
    area: Rect,
    gap: f32,
) -> (Rect, (usize, usize), usize) {
    let ((w, h), grid, cap) = row_cells(n, cells, area.w);
    let x = row_box_x(anchor, w, area);
    (Rect::new(x, (anchor.y - gap - h).max(area.y), w, h), grid, cap)
}

/// A side dock's folder open: its members in a COLUMN (a second dock
/// beside the dock), centred on `anchor` (the dock icon) and `gap` past the
/// dock's `band` on the screen's side, kept inside `area`'s height.
pub fn col_box(band: Rect, anchor: Rect, n: usize, (cell_w, cell_h): (f32, f32), area: Rect, gap: f32, left: bool) -> (Rect, (usize, usize), usize) {
    let cap = ((area.h / cell_h.max(1.0) + 0.001).floor() as usize).max(1);
    let rows = n.clamp(1, cap);
    let h = rows as f32 * cell_h;
    let y = (anchor.y + anchor.h / 2.0 - h / 2.0).clamp(area.y, (area.y + area.h - h).max(area.y));
    let x = if left { band.x + band.w + gap } else { band.x - gap - cell_w };
    (Rect::new(x, y, cell_w, h), (1, rows), cap)
}

/// An open box's left edge: `w` wide, centred on `anchor`, inside `area`.
pub fn row_box_x(anchor: Rect, w: f32, area: Rect) -> f32 {
    (anchor.x + anchor.w / 2.0 - w / 2.0).clamp(area.x, (area.x + area.w - w).max(area.x))
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
    if layout.sideways() {
        // A side dock: its band, each icon's row across it.
        if layout.on_dock(pos) {
            for (i, slot) in layout.dock_slots.iter().enumerate() {
                if py >= slot.y && py < slot.y + slot.h {
                    return Some(Hit::DockIcon(i));
                }
            }
        }
    } else if py >= layout.card_top && py < layout.dock_hit_bottom {
        for (i, slot) in layout.dock_slots.iter().enumerate() {
            if px >= slot.x && px < slot.x + slot.w {
                return Some(Hit::DockIcon(i));
            }
        }
    }
    if !search_open && layout.search_btn.contains(pos) {
        return Some(Hit::SearchButton);
    }
    if let Some(i) = layout.controls.iter().position(|r| r.contains(pos)) {
        return Some(Hit::Control(i));
    }
    // The open box: resolve the member slot under the pointer (clicks
    // inside are inert/launch; only outside closes).
    if let Some(b) = layout.open_box {
        if b.contains(pos) {
            return Some(Hit::OpenBoxCell(open_box_slot(b, layout.open_box_grid, pos)));
        }
    }
    for (s, sec) in layout.sections.iter().enumerate() {
        if sec.n_pages > 0 && sec.viewport.contains(pos) {
            // Derive scaled cell dimensions from the already-scaled layout geometry.
            let cell_w = sec.viewport.w / sec.cols.max(1) as f32;
            // (Not viewport.h / rows: a trimmed Apps viewport is shorter.)
            let cell_h = cell_w * GRID_CELL_H / GRID_CELL_W;
            // Pages going DOWN (a side dock's box): the page under the
            // pointer, then its row and column as on a page across.
            if sec.vertical {
                let page_h = sec.viewport.h.max(1.0);
                // (A side box's cells are narrower than tall: its rows by its height.)
                let cell_h = page_h / sec.rows.max(1) as f32;
                let adjusted_y = pos.1 - sec.viewport.y + sec.scroll;
                let page = (adjusted_y / page_h).floor() as usize % sec.n_pages.max(1);
                let in_page = adjusted_y - (adjusted_y / page_h).floor() * page_h;
                let y = match layout.apps_part.filter(|_| s == SECTION_APPS) {
                    Some(part) => match part.rest_y(sec.viewport.y, cell_h, sec.viewport.y + in_page) {
                        Some(y) => y - sec.viewport.y,
                        None => continue,
                    },
                    None => in_page,
                };
                let row = (y / cell_h).floor() as usize;
                let row_start = page * sec.cols * sec.rows + row * sec.cols;
                let col_f = (pos.0 - sec.viewport.x - sec.row_shift(row_start)) / cell_w;
                let col = col_f.floor() as usize;
                if col_f >= 0.0 && col < sec.cols && row < sec.rows {
                    let i = page * sec.cols * sec.rows + row * sec.cols + col;
                    if i < sec.cells {
                        if s == SECTION_APPS {
                            return apps_slots.iter().position(|&slot| slot == i).map(|j| Hit::GridCell(s, j));
                        }
                        return Some(Hit::GridCell(s, i));
                    }
                }
                continue;
            }
            // Adjust for horizontal page scroll; pages wrap cyclically.
            let adjusted_x = pos.0 - sec.viewport.x + sec.scroll;
            let page_w = sec.viewport.w.max(1.0);
            let page = (adjusted_x / page_w).floor() as usize % sec.n_pages.max(1);
            // Apps rows parted around an open box: back to where they rest.
            let y = match layout.apps_part.filter(|_| s == SECTION_APPS) {
                Some(part) => match part.rest_y(sec.viewport.y, cell_h, pos.1) {
                    Some(y) => y,
                    None => continue,
                },
                None => pos.1,
            };
            let row = ((y - sec.viewport.y) / cell_h).floor() as usize;
            let row_start = page * sec.cols * sec.rows + row * sec.cols;
            let col_f = (adjusted_x - page as f32 * page_w - sec.row_shift(row_start)) / cell_w;
            let col = col_f.floor() as usize;
            if col_f >= 0.0 && col < sec.cols && row < sec.rows {
                let cells_per_page = sec.cols * sec.rows;
                let i = page * cells_per_page + row * sec.cols + col;
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
    /// The ghost is currently hovering the dock band — it sizes to the
    /// dock icons there, and back to the grid icon over the grid, so a
    /// grabbed icon always matches whatever row it is over.
    pub on_dock: bool,
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

/// A grid name too long for its cell's one line (Max, 2026-10-22: "dos
/// líneas cuando sea necesario" — "YouTube Music" read as "YouTube" cut at a
/// word, "Google Calendar" as "Google"): broken between words onto two, and
/// on one with "…" where the second has no room (`App::fitted_names`). Not a
/// file's: those are cut in the middle, their ending kept (`tile_label`).
#[derive(Debug, Clone, PartialEq)]
pub struct NameFit {
    pub one: String,
    pub two: (String, String),
}

/// How far under the first line a grid name's second sits: a little closer
/// than a line, so it stays clear of the next row's icons.
const SECOND_LINE: f32 = 14.0;

/// A lone app's RECENTS as drawn (`app_actions.rs`): the page shown, the
/// shapes of their pictures, and the slide between pages.
#[derive(Debug, Clone, Default)]
pub struct RecentsDraw {
    /// The entries of the page shown.
    pub cells: Vec<usize>,
    /// Each preview's own shape (entry, width / height): drawn WHOLE, a
    /// little bigger than an icon (Max, 2026-10-23: "forma libre… no muy
    /// chicas, como las miniaturas de los archivos, un poco más grandes").
    pub shapes: Vec<(usize, f32)>,
    /// Where the page shown stands, in widths of the row (0 in place, 1 a
    /// whole row to the right), and which way the pages went (+1 the next).
    pub slide: f32,
    pub dir: f32,
    /// The page that went, as it was, sliding out (`Leaving`).
    pub leaving: Vec<Leaving>,
}

/// A recent of the page that went, drawn as it slides out.
#[derive(Debug, Clone)]
pub struct Leaving {
    /// Its display slot in the grid, its picture's layer, its name.
    pub slot: usize,
    pub layer: u32,
    pub name: String,
    pub aspect: Option<f32>,
}

/// How high a recent's preview stands, of an icon; how wide it may be, of a
/// cell (less a margin).
const PREVIEW_H: f32 = 1.3;
const PREVIEW_MARGIN: f32 = 3.0;

/// A preview of shape `aspect` drawn whole, its bottom on the icons' line
/// at `bottom`, centred on `cx`: the square texture's rect (the picture
/// stands centred in it, `thumbs`).
fn preview_rect(cx: f32, bottom: f32, aspect: f32, cell_w: f32, icon: f32) -> Rect {
    let (bw, bh) = ((cell_w - PREVIEW_MARGIN).max(icon), icon * PREVIEW_H);
    let (pw, ph) = if aspect >= bw / bh { (bw, bw / aspect) } else { (bh * aspect, bh) };
    let side = pw.max(ph);
    Rect::new(cx - side / 2.0, bottom - ph / 2.0 - side / 2.0, side, side)
}

/// Per-frame dynamic inputs to scene assembly.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInput<'a> {
    /// The grid cells whose names need two lines, by entry (`NameFit`).
    pub names: Option<&'a std::collections::HashMap<usize, NameFit>>,
    /// A lone app's recents: their pictures' shapes and their pages' slide.
    pub recents: Option<&'a RecentsDraw>,
    /// A lone app's recents' page shown and how many, when more than one:
    /// their dots under their line (`app_actions.rs`).
    pub recent_pages: Option<(usize, usize)>,
    /// Item under the pointer (hover highlight).
    pub hover: Option<Hit>,
    /// Shaped pixel width of the live query (the caret anchor).
    pub query_px: f32,
    /// The search pill's size, type and hover.
    pub search_pill: SearchPill,
    /// The System row's settings and modules (label, shaped width), in
    /// `Layout::controls` order, and each pill's eased hover.
    pub controls: &'a [(&'static str, f32)],
    pub control_lift: &'a [f32],
    /// The lone app's ACTION cells (`app_actions.rs`): each one's entry and
    /// the texture layer of its symbol; and the one asking to be sure.
    pub act_cells: &'a [(usize, u32)],
    pub act_asking: Option<usize>,
    /// The action cell whose name may spread over `WIDE_LABEL` cells (a
    /// site's search, alone in its row).
    pub act_wide: Option<usize>,
    /// The lone app's recents' title ("Recent in VLC"), drawn in the room
    /// opened under its row.
    pub recent_title: Option<&'a str>,
    /// THE DEEP FILE SEARCH (`deep_search.rs`): what its shelves, chips and
    /// pill draw, while it is up.
    pub deep: Option<DeepDraw<'a>>,
    /// FAST LAUNCH's bubble and icons, while it is up or fading.
    pub fast: Option<FastDraw<'a>>,
    pub quick: Option<QuickDraw<'a>>,
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
    /// Smoothed per-slot dock magnification scales (1.0 = rest), eased
    /// in the frame loop toward the pointer-derived targets (quick
    /// bloom, slower melt — see `DOCK_MAG_GROW_RATE`). The dock draws
    /// exactly these; empty = every icon at rest.
    pub dock_mag: &'a [f32],
    /// Live plate colour for app-icon squircle plates (see
    /// [`IconInst::plate`]) — the surface's adaptive frost, eased in the
    /// frame loop alongside the fill. Zero alpha (the `Default`) draws no
    /// plates at all (`icon_plate` off, tests).
    pub plate: [f32; 4],
    /// Launch bounce: (entry index, upward offset in px).
    pub bounce: Option<(usize, f32)>,
    /// The whole dock's bounce: every icon on it lifted by this, px.
    pub dock_hop: f32,
    /// Live search query (empty shows the placeholder).
    pub query: &'a str,
    /// Auto-selected grid position (best match while searching), as
    /// (section, cell index within that section's visible list). THE ONE
    /// highlight of the grid: the pointer's hover is drawn only while
    /// nothing is selected (the pointer moving onto a cell selects it).
    pub selected: Option<(usize, usize)>,
    /// The pointer took the selection, not the keys: drawn as a hover is.
    pub selected_soft: bool,
    /// Search box expansion: 0.0 = compact circle button, 1.0 = full pill.
    pub search_expand: f32,
    /// Which entries are using placeholder tiles (no resolved icon file).
    /// Aligned with the `entries` slice passed to `scene()`.
    pub placeholders: &'a [bool],
    /// What each entry is, aligned with `entries` (empty: all apps). Only
    /// an app's icon wears a plate.
    pub kinds: &'a [crate::apps::EntryKind],
    /// Texture layer per entry. Normally the entry's own index; dynamic
    /// file-search results borrow a generic asset icon's layer. Empty
    /// slice = identity (entry index is the layer).
    pub layers: &'a [u32],
    /// Dock display order: maps slot position → entry index. Empty slice
    /// renders no dock icons (safe default for tests / pre-load frames).
    pub dock_order: &'a [usize],
    /// Per-slot running flag, aligned with `dock_order`: `true` draws the
    /// macOS-style running indicator dot under that icon. Short/empty
    /// slice ⇒ no dots.
    pub dock_running: &'a [usize],
    /// Dock slot at which the running-but-unpinned zone begins: a thin
    /// divider is drawn just left of it. `None` (or 0) draws no divider.
    pub dock_divider: Option<usize>,
    /// Corner app-icon badge per minimized tile, in dock-tail order (index k
    /// = the k-th minimized tile, `slot - dock_min_start`): the texture layer
    /// of the app matching the window's class, or `None` for no badge. Short/
    /// empty slice ⇒ no badges.
    pub min_badges: &'a [Option<u32>],
    /// Aspect ratio per minimized tile (same order as `min_badges`), so the
    /// draw fits each tile into its box exactly as `layout()` sized its slot.
    pub min_aspects: &'a [f32],
    /// Active drag for ghost icon and insertion indicator; `None` at rest.
    pub drag: Option<DragFrame>,
    /// Recycle-bin reaction (0 = shut/grey at rest, 1 = open/red while an app
    /// is being dragged): drives the bin tile's lid openness and red tint.
    pub trash_react: f32,
    /// Recycle-bin hover-grow (0 = rest size, 1 = a dragged icon is over it):
    /// swells the bin tile to acknowledge the drop it would accept.
    pub trash_hover: f32,
    /// The dock bin's states (`bin_motion`).
    pub bin: crate::bin_motion::Pose,
    /// Golem Files' launcher: its first figure layer and how far its folders
    /// are out (`files_icon`); `None` draws its theme icon.
    pub files_fig: Option<(u32, f32)>,
    /// Its folders' lifts in the click's wave (back, middle, front; 64-grid
    /// units, up).
    pub files_wave: [f32; 3],
    /// The apps button's beat (1 = rest; `apps_icon`).
    pub apps_beat: f32,
    /// Its stencil layer: its plate cut by its squares while it holds pins.
    pub apps_stencil: Option<u32>,
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
    /// Install progress per entry, aligned with `entries`: `0.0..=1.0`
    /// draws a macOS-style circular progress ring over an installing tile;
    /// a negative value (the default) means no ring. Empty slice = none.
    pub progress: &'a [f32],
    /// Shine-sweep progress per entry, aligned with `entries`: `0.0..=1.0`
    /// sweeps a glass reflection across a just-finished install's icon (after
    /// its ring fills); a negative value means no shine. Empty slice = none.
    pub shine: &'a [f32],
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
    /// The card is the control panel: draw its sections and search
    /// empty (the dock band stays).
    pub panel: bool,
    /// The control panel's field this frame (see `panel.rs`).
    pub panel_draw: Option<&'a crate::panel::PanelDraw>,
    /// The pictures of the panel's open setting: where, which layer.
    pub panel_images: &'a [(Rect, u32, f32)],
    /// Apps ↔ settings: 0 = the apps, 1 = the panel. Between, the settings
    /// field rises in from the card's bottom edge pushing the apps up and
    /// out under the dock (and back down the other way).
    pub panel_mix: f32,
    /// Animated display slot of each member (`page * open_box_cap +
    /// within`, parallel to `open_box_members`): box pages may be
    /// under-full, so slots — not list positions — decide where a member
    /// draws. Fractional mid-glide (the box's make-room reflow, computed
    /// in `draw` like the grid's `apps_slide`).
    pub open_box_disp: &'a [f32],
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
    /// Adaptive card fill — the dock's twin of the OPTIONS bar's colour
    /// match: the flush window's colour, its sampled frosted backdrop, or
    /// (before the first sample) the static theme background. See
    /// `App::dock_surface`. Replaces every `config.theme.background_rgba()`
    /// use in this file.
    pub dock_bg: [f32; 4],
    /// Adaptive ink measured against `dock_bg` — dark on a bright card,
    /// light on a dark one. Replaces every `config.theme.text_rgba()` use in
    /// this file (and is also the renderer's default label colour, see
    /// `App::draw`).
    pub dock_ink: [f32; 4],
    /// Adaptive hover wash, replacing `config.theme.highlight_rgba()`.
    pub dock_highlight: [f32; 4],
    /// How far the card wears the toned glass (0..1, `App::dock_tone`): as
    /// dense resting as open, its shadow round the top and sides lighter.
    pub dock_tone: f32,
    /// The open card's ground at its worst as the screen shows it (sRGB;
    /// alpha 0: unknown): what its faint words are laid on (`faded_on`).
    pub dock_seen: [f32; 4],
    /// How dense the open card must be for its words to read over anything
    /// (`options::dense_enough` on `worst_behind`).
    pub dock_open_alpha: f32,
}

/// FAINT WORDS, SOLID (the legibility audit, 2026-10-23): `ink` faded `a` of
/// the way from what the screen shows under it (`seen`, sRGB) — laid as a
/// solid colour, since a faded ink on our see-through card is lifted toward
/// the card by the compositor's blend (a section title at .3 read 1.4:1 on
/// the light dock) — and no fainter than `floor` on it. `seen` unknown
/// (alpha 0): the fade stays an alpha.
pub(crate) fn faded_on(ink: [f32; 4], seen: [f32; 4], a: f32, floor: f32) -> [f32; 4] {
    if seen[3] <= 0.0 {
        return [ink[0], ink[1], ink[2], ink[3] * a];
    }
    let mut k = a.clamp(0.0, 1.0);
    loop {
        let c = [0, 1, 2].map(|i| seen[i] + (ink[i] - seen[i]) * k);
        let c = [c[0], c[1], c[2], 1.0];
        if k >= 1.0 || crate::options::srgb_contrast(c, seen) >= floor {
            return [c[0], c[1], c[2], ink[3]];
        }
        k = (k + 0.02).min(1.0);
    }
}

/// The open box's slot under `pos` — the member's place on its page, the
/// rows read left to right, top to bottom.
pub fn open_box_slot(b: Rect, (cols, rows): (usize, usize), pos: (f32, f32)) -> usize {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let col = (((pos.0 - b.x) / (b.w / cols as f32)).floor().max(0.0) as usize).min(cols - 1);
    let row = (((pos.1 - b.y) / (b.h / rows as f32)).floor().max(0.0) as usize).min(rows - 1);
    row * cols + col
}

/// Rim colour for a plate-coloured tile: the fill's hue lifted toward
/// white with a floor of visibility — the CPU-side twin of the icon
/// shader's plate hairline (icon.wgsl) so the Recycle Bin tile's border
/// matches the app-icon plates around it.
fn plate_rim(fill: [f32; 4]) -> [f32; 4] {
    [
        lerp(fill[0], 1.0, 0.15),
        lerp(fill[1], 1.0, 0.15),
        lerp(fill[2], 1.0, 0.15),
        (fill[3] + 0.06).min(1.0),
    ]
}

/// How far the Bin's tile warms toward red under a drag (1 = full red).
const TRASH_RED: f32 = 0.5;

/// The Recycle Bin tile's rounded-rect fill colour for reaction `react`. At
/// rest it wears the live adaptive PLATE colour — the same hue-shifted frost
/// every app icon's squircle plate wears (Max, 2026-09-10: "add the color to
/// the recycle can plate too") — so the bin sits in the row as one of them;
/// it lerps to a translucent red as an app arms the drop.
fn trash_tile_color(plate: [f32; 4], react: f32) -> [f32; 4] {
    // Only part of the way to red (Max, 2026-10-01: "a little weaker").
    let r = react.clamp(0.0, 1.0) * TRASH_RED;
    // The title bars' close-button red, #FF2E2E (waveview's traffic
    // lights), in the renderer's linear space.
    let red = [1.0, crate::options::srgb_to_linear(46.0 / 255.0), crate::options::srgb_to_linear(46.0 / 255.0), 0.66];
    [
        lerp(plate[0], red[0], r),
        lerp(plate[1], red[1], r),
        lerp(plate[2], red[2], r),
        lerp(plate[3], red[3], r),
    ]
}

/// The dock bin's tile (`bin_motion`): the plate washed toward the traffic
/// lights' ORANGE (#FF9500, minimize) while something is in hand, toward
/// their RED (#FF2E2E, close) over it.
fn bin_tile_color(plate: [f32; 4], orange: f32, red: f32) -> [f32; 4] {
    let lin = |c: f32| crate::options::srgb_to_linear(c / 255.0);
    let o = [1.0, lin(149.0), lin(0.0), 0.66];
    let r = [1.0, lin(46.0), lin(46.0), 0.66];
    let (ko, kr) = (orange.clamp(0.0, 1.0) * 0.6, red.clamp(0.0, 1.0) * 0.72);
    std::array::from_fn(|i| lerp(lerp(plate[i], o[i], ko), r[i], kr))
}

/// The dock bin's can and lid where `bin_motion` says, on `rect` (the
/// figure's 100 units across it): each turned about its own pivot.
fn push_bin_posed(scene: &mut Scene, rect: Rect, layer: u32, pose: &crate::bin_motion::Pose) {
    let u = rect.w / 100.0;
    // A quad turned by `deg` and scaled (sx, sy) about the pivot (px, py),
    // then moved by (dx, dy): its middle goes where the pivot sends it.
    let mut part = |layer: u32, (px, py): (f32, f32), deg: f32, (sx, sy): (f32, f32), (dx, dy): (f32, f32)| {
        let (s, c) = deg.to_radians().sin_cos();
        let (ox, oy) = ((50.0 - px) * sx, (50.0 - py) * sy);
        let (mx, my) = (px + ox * c - oy * s + dx, py + ox * s + oy * c + dy);
        let (w, h) = (rect.w * sx, rect.h * sy);
        scene.icon_rot.resize(scene.icons.len(), 0.0);
        scene.icons.push(IconInst {
            rect: Rect::new(rect.x + mx * u - w / 2.0, rect.y + my * u - h / 2.0, w, h),
            layer,
            tint: [0.0; 4],
            ring: -1.0,
            plate: NO_PLATE,
        });
        scene.icon_rot.push(deg.to_radians());
    };
    let (rot, sx, sy) = pose.can;
    part(layer, (50.0, 82.0), rot, (sx, sy), (0.0, 0.0));
    let (dx, dy, rot) = pose.lid;
    part(layer + 1, (50.0, 30.0), rot, (1.0, 1.0), (dx, dy));
}

/// Scale `r` by `scale` about its bottom-centre — the dock grow anchor, so a
/// swelling icon rises off the bar instead of sinking into it.
fn grow_bottom(r: Rect, scale: f32) -> Rect {
    let (w, h) = (r.w * scale, r.h * scale);
    Rect::new(r.x + r.w / 2.0 - w / 2.0, r.y + r.h - h, w, h)
}

/// [`grow_bottom`] for any dock: the icon grows away from the screen's edge
/// its dock stands on, held at that edge.
fn grow_from_edge(r: Rect, scale: f32, side: Side) -> Rect {
    let (w, h) = (r.w * scale, r.h * scale);
    match side {
        Side::Bottom => grow_bottom(r, scale),
        Side::Left => Rect::new(r.x, r.y + r.h / 2.0 - h / 2.0, w, h),
        Side::Right => Rect::new(r.x + r.w - w, r.y + r.h / 2.0 - h / 2.0, w, h),
    }
}

/// The Recycle Bin's metal figure: the can at `layer`, its lid at
/// `layer + 1` (uploaded together, see `App::bin_layer`), the lid lifting
/// a little as `react` (a drag toward the bin) rises.
fn push_bin(icons: &mut Vec<IconInst>, rect: Rect, layer: u32, react: f32) {
    let lift = rect.h * BIN_LID_LIFT * react.clamp(0.0, 1.0);
    for (layer, dy) in [(layer, 0.0), (layer + 1, -lift)] {
        icons.push(IconInst {
            rect: Rect::new(rect.x, rect.y + dy, rect.w, rect.h),
            layer,
            tint: [0.0; 4],
            ring: -1.0,
            plate: NO_PLATE,
        });
    }
}

/// How far the bin's lid rises under a drag, as a share of the icon.
const BIN_LID_LIFT: f32 = 0.20;

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
        names,
        recents,
        recent_pages,
        hover,
        query_px,
        search_pill,
        controls,
        control_lift,
        act_cells,
        act_asking,
        act_wide,
        recent_title,
        deep,
        fast,
        quick,
        stretch,
        dock_tooltip,
        alpha,
        pointer,
        mag_amount,
        dock_mag,
        plate,
        bounce,
        dock_hop,
        query,
        selected,
        selected_soft,
        search_expand,
        placeholders,
        kinds,
        layers,
        dock_order,
        dock_running,
        dock_divider,
        min_badges,
        min_aspects,
        drag,
        trash_react,
        trash_hover,
        bin,
        files_fig,
        files_wave,
        apps_beat,
        apps_stencil,
        install_hint,
        busy,
        failed,
        installing,
        launching,
        progress,
        shine,
        group_minis,
        apps_group,
        apps_slide,
        drag_hidden,
        dock_hidden,
        dock_slide,
        group_expand,
        group_origin,
        open_box_members,
        panel,
        panel_draw,
        panel_images,
        panel_mix,
        open_box_disp,
        open_box_pages,
        box_scroll,
        box_drag,
        breath_offset,
        card_push,
        box_push,
        card_open,
        card_shadow,
        box_over_dock,
        dock_bg,
        dock_ink,
        dock_highlight,
        dock_tone,
        dock_seen,
        dock_open_alpha,
    } = *frame;
    let layer_of = |i: usize| layers.get(i).copied().unwrap_or(i as u32);
    // Plate for an app entry's icon: the live adaptive colour. Only an
    // app's (Max, 2026-10-13: "on the dock only icons get the plate, not
    // folders or thumbnails"): a folder, a file's thumbnail and the
    // `asset-*` file-type carriers stand bare wherever they are — pinned to
    // the dock, in a folder's stack over it, in the Files row (matching
    // the rasterizer, which never plated them either).
    let plate_of = |i: usize| {
        let bare = matches!(kinds.get(i), Some(crate::apps::EntryKind::File | crate::apps::EntryKind::Asset))
            || entries.get(i).is_some_and(|e| e.id.starts_with("asset-"));
        if bare {
            NO_PLATE
        } else {
            plate
        }
    };
    // An action asking to be sure (`app_actions.rs`): its plate in the red
    // of the menus, the one white words read on (`desktop_menu::danger_on`).
    let asking_plate = {
        let d = crate::palette::danger_srgb();
        let own = [
            crate::options::srgb_to_linear(d[0]),
            crate::options::srgb_to_linear(d[1]),
            crate::options::srgb_to_linear(d[2]),
            1.0,
        ];
        let red = crate::desktop_menu::danger_on(own, [1.0; 4], false).0;
        [red[0], red[1], red[2], 0.92]
    };
    // Scaled drawing metrics — mirrors what layout() received.
    let dock_icon = DOCK_ICON * icon_scale;
    let dock_slot = DOCK_SLOT * icon_scale;
    let grid_icon = GRID_ICON * icon_scale;
    let grid_icon_top = GRID_ICON_TOP * icon_scale;
    let box_tile = BOX_TILE * icon_scale;
    let grid_cell_h = GRID_CELL_H * icon_scale;
    // The magnified open box's current rect (grows from the tile into its
    // rest square). Used to draw the box and to hide the grid labels it
    // covers — otherwise those names paint over it in the later text pass.
    let open_box_rect = match (open_box_members.is_empty(), group_origin, layout.open_box) {
        // In the grid: the line under the tile's row, the grid's width,
        // opening from its top edge down as the rows below make room.
        (false, Some(_), Some(to)) if !box_over_dock => {
            let vp = layout.sections[SECTION_APPS].viewport;
            Some(Rect::new(vp.x, to.y, vp.w, to.h * group_expand.clamp(0.0, 1.0)))
        }
        // Over the dock: the same run upwards, from its bottom edge — the
        // one next to its icon. A plate like the dock's, with the dock's
        // own padding at the ends.
        // Beside a SIDE dock: a column, opening out of its edge by the dock.
        (false, Some(_), Some(to)) if layout.sideways() => {
            let w = to.w * group_expand.clamp(0.0, 1.0);
            let pad = DOCK_PAD_X * icon_scale;
            let x = if layout.side == Side::Left { to.x } else { to.x + to.w - w };
            Some(Rect::new(x, to.y - pad, w, to.h + 2.0 * pad))
        }
        (false, Some(_), Some(to)) => {
            let h = to.h * group_expand.clamp(0.0, 1.0);
            let pad = DOCK_PAD_X * icon_scale;
            Some(Rect::new(to.x - pad, to.y + to.h - h, to.w + 2.0 * pad, h))
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
    // A side dock: its box grows out of the edge, clipped on both axes to
    // the card less the dock's band.
    let side = layout.side;
    let sideways = layout.sideways();
    let reveal_rect = if sideways {
        layout.box_area(dock_h)
    } else {
        Rect::new(
            layout.card_x,
            layout.card_top + dock_h,
            layout.card_w,
            (card_h - dock_h).max(0.0),
        )
    };
    let reveal_top = reveal_rect.y;
    let reveal_bottom = reveal_rect.y + reveal_rect.h;
    let reveal_clip = |r: Rect| -> Rect {
        if sideways {
            return r.intersect(&reveal_rect);
        }
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
    // (On the dock, the whole dock's bounce on top.)
    let dock_lift = |i: usize| lift(i) + dock_hop;

    // Dock row: per-icon magnification scales, then spread visual centers.
    // Hit-boxes stay at fixed slot positions; only drawn positions spread.
    // The scales are the frame loop's *smoothed* values (see
    // `dock_mag_target` / `FrameInput::dock_mag`), never raw pointer math —
    // the crest blooms and melts instead of snapping per pointer event.
    //
    // Computed HERE, above the card, because the card is built from these
    // same numbers: the row's growth drives the card's growth (Max,
    // 2026-09-13: "the icons move to the sides and the card too, same
    // movement"). Sharing the arithmetic is what welds them — a second
    // eased value tuned to look the same would drift apart the moment
    // either side is retuned. The card takes a *fraction* of the travel
    // (`DOCK_SPREAD_FOLLOW`), which is a scale on one shared motion, not a
    // second motion: same curve, same moment, shorter reach.
    let dock_scales: Vec<f32> = (0..layout.dock_slots.len())
        .map(|i| dock_mag.get(i).copied().unwrap_or(1.0))
        .collect();
    // Per-slot VISUAL width once magnified. The BASE width is uniform
    // `dock_slot` in the normal zone and the aspect-derived tile width
    // layout() baked into `.w` for the minimized tail — reading it back keeps
    // the drawn tiles and their hit-boxes on one source of truth. GROW is the
    // horizontal reach of magnification: a normal icon grows by `dock_icon`;
    // a thumbnail grows by its own (un-padded) picture width, so it keeps its
    // shape while it swells.
    let min_tile_pad = MIN_TILE_PAD * icon_scale;
    let dock_vw: Vec<f32> = layout
        .dock_slots
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // (Beside a side dock a minimized window's tile is a square.)
            let grow = if i >= layout.dock_min_start && !sideways {
                (r.w - min_tile_pad).max(0.0)
            } else {
                dock_icon * layout.dock_slot_f.get(i).copied().unwrap_or(1.0)
            };
            layout.slot_span(r).1 + grow * (dock_scales[i] - 1.0)
        })
        .collect();
    // Each magnified slot widens by the pixels its icon/thumbnail grew,
    // pushing neighbours apart. Row stays centered as a whole.
    let total_vw: f32 = dock_vw.iter().sum();
    let dock_vcx: Vec<f32> = {
        // The minimized-tile gap is part of the row's visual width and sits
        // before `dock_min_start` — same offset layout() baked into the
        // hit-test slots, so drawn centers and hit-boxes stay aligned.
        // (Along a side dock: down the screen's middle.)
        let mid = if sideways { surface.1 / 2.0 } else { w / 2.0 };
        let mut x = mid - (total_vw + layout.dock_min_gap) / 2.0;
        let mut centers = Vec::with_capacity(dock_vw.len());
        for (i, &vw) in dock_vw.iter().enumerate() {
            if i == layout.dock_min_start {
                x += layout.dock_min_gap;
            }
            centers.push(x + vw / 2.0);
            x += vw;
        }
        centers
    };
    // How much wider the magnified row is than its resting width. The row
    // is centered, so each of its outer edges travels half of this.
    let row_base_total: f32 = layout.dock_slots.iter().map(|r| layout.slot_span(r).1).sum();
    let row_spread = (total_vw - row_base_total).max(0.0);
    // What the CARD takes of that travel — the same motion at
    // `DOCK_SPREAD_FOLLOW` of the reach.
    //
    // Clamped to the surface: the resting basin leaves only
    // `BASIN_MARGIN_X` a side, and a magnification tuned far past the
    // default would otherwise push the card's rounded corners off the
    // fixed-size surface and cut them square.
    let card_spread = (row_spread * DOCK_SPREAD_FOLLOW).min(if sideways {
        (surface.1 - layout.card_h).max(0.0)
    } else {
        (w - layout.card_w).max(0.0)
    });

    // Card background: the AGUA silhouette — wide basin at rest,
    // gathered to the box width risen, spilling on the landing.
    // `breath_offset` expands all four edges equally (card center fixed,
    // icons stay anchored); positive = exhale (card grows), negative = inhale.
    // `card_spread` widens it in step with the magnified row (above).
    // Per-edge jelly: each spring moves only its own edge.
    // (left, right, top, bottom) offsets; positive = inward.
    // Combined with breath (uniform expansion from all edges).
    let (jl, jr, jt, jb) = card_push;
    // (A side dock's card spreads along its height.)
    let (spread_x, spread_y) = if sideways { (0.0, card_spread) } else { (card_spread, 0.0) };
    let card_rect = Rect::new(
        layout.card_x - spread_x / 2.0 - breath_offset + jl,
        layout.card_top - spread_y / 2.0 - breath_offset + jt,
        layout.card_w + spread_x + 2.0 * breath_offset - jl + jr,
        card_h + spread_y + 2.0 * breath_offset - jt + jb,
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
            color: [0.0, 0.0, 0.0, card_shadow.clamp(0.0, 1.0) * crate::options::shell_shade()],
            edges: [
                DOCK_SHADOW_ALPHA_TOP * (1.0 - DOCK_SHADOW_TONED_CUT * dock_tone),
                DOCK_SHADOW_ALPHA_BOTTOM,
                DOCK_SHADOW_ALPHA_SIDE * (1.0 - DOCK_SHADOW_TONED_CUT * dock_tone),
                DOCK_SHADOW_ALPHA_SIDE * (1.0 - DOCK_SHADOW_TONED_CUT * dock_tone),
            ],
        });
    }
    // The card fill: the resting dock wears only `DOCK_REST_FILL` of the
    // theme's translucency, gathering the full alpha as `card_open` rises
    // (eased upstream, so the ride is gradual). Only the card — the group
    // box panel, tooltip pills, and icon plates keep their own fills.
    // Reduce-transparency keeps its forced-opaque card in every state.
    let mut card_fill = dock_bg;
    if !config.accessibility.reduce_transparency {
        // Open, the card reaches `theme.open_opacity` when one is set (the
        // light tier's denser box), else the background's own alpha.
        // (Open, dense enough for its words over anything behind.)
        let open_a = config.theme.open_alpha(card_fill[3]).max(dock_open_alpha);
        // (The toned glass rests as dense as it opens.)
        let rest = lerp(DOCK_REST_FILL, 1.0, dock_tone);
        card_fill[3] = lerp(card_fill[3] * rest, open_a.max(card_fill[3] * dock_tone), card_open);
        // (The Shell's Transparency, the owner's, from there.)
        card_fill[3] = crate::options::shell_glass(card_fill[3]);
    }
    scene.rects.push(RectInst {
        rect: card_rect,
        radius: card_radius,
        color: card_fill,
        glass: 1.0,
        border: 0.0,
    });
    // The same subtle rim the icon plates wear (`plate_rim`, Max
    // 2026-09-10: "add the same border on the dock"), traced along the
    // card edge. Derived from the live fill, so it rests as faint as the
    // resting veil and firms up with the open ramp.
    scene.rects.push(RectInst {
        rect: card_rect,
        radius: card_radius,
        color: plate_rim(card_fill),
        glass: 0.0,
        border: 1.0,
    });

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
                let fg = dock_ink;
                // Normally the line sits at the midpoint. But when this seam
                // is the minimized-tile boundary, the min-gap opens on the
                // RIGHT of it: the separator belongs to the pinned group, so
                // anchor it just past the left icon and let the whole gap
                // fall between it and the thumbnail (Max, 2026-09-16: "more
                // space between the separator and the thumbnail").
                let x_mid = if div == layout.dock_min_start {
                    // MIN_TILE_PAD past the pinned zone's right edge, so the
                    // divider sits the same distance from the trash as the
                    // thumbnails sit from each other (DOCK_MIN_GAP then leaves
                    // an equal gap after it).
                    left + dock_slot / 2.0 + MIN_TILE_PAD * icon_scale
                } else {
                    (left + right) / 2.0
                };
                // (Across a side dock's column: a short level line.)
                let rect = if sideways {
                    let inset = slot.w * 0.22;
                    Rect::new(slot.x + inset, x_mid - DOCK_DIVIDER_W / 2.0, (slot.w - inset * 2.0).max(1.0), DOCK_DIVIDER_W)
                } else {
                    Rect::new(x_mid - DOCK_DIVIDER_W / 2.0, slot.y + inset, DOCK_DIVIDER_W, (slot.h - inset * 2.0).max(1.0))
                };
                scene.rects.push(RectInst {
                    rect,
                    radius: DOCK_DIVIDER_W / 2.0,
                    color: [fg[0], fg[1], fg[2], 0.22],
                    glass: 0.0,
                    border: 0.0,
                });
            }
        }
    }
    // No hover wash on the dock (removed, Max 2026-09-10): the magnification
    // wave and tooltip are the hover feedback; the wash read as a smudge.
    // Dock icons: slot index → entry index via dock_order. `dock_slide`
    // parts the row around a hovering drag; a dock-origin drag's own
    // icon is hidden (its ghost is in hand).
    // THE OVERFLOW AS A LINE (Max, 2026-10-16): the pins that do not fit
    // go on, a slot apart, BEHIND the apps button — which is drawn over them
    // with its plate cut by its squares while it holds one — and are cut
    // off at the row's left edge and at the button's right side; never one
    // over another, never shrunk. Those at the window's ends are drawn after
    // the loop, cut (`edge`), the button over them (`apps_top`).
    let overflow = layout.dock_overflow;
    let mut edge_pins: Vec<(usize, usize, f32)> = Vec::new();
    let mut apps_top: Vec<IconInst> = Vec::new();
    let mut apps_drawn: Option<Rect> = None;
    for slot in 0..layout.dock_slots.len() {
        let Some(&entry_idx) = dock_order.get(slot) else {
            break;
        };
        if dock_hidden == Some(entry_idx) {
            continue;
        }
        if let Some(ov) = overflow.filter(|o| slot < o.apps_slot) {
            let s = slot as f32 - ov.off;
            if s < 0.0 || s > ov.cap as f32 - 1.0 {
                if s > -1.0 && s < ov.cap as f32 + 1.0 {
                    edge_pins.push((slot, entry_idx, s));
                }
                continue;
            }
        }
        // A pin folded into the overflow tile (or slid off the left end)
        // shrinks away with its slot.
        let shown = layout.dock_slot_f.get(slot).copied().unwrap_or(1.0);
        if shown < 0.02 {
            continue;
        }
        let slot_rect = &layout.dock_slots[slot];
        let vcx = dock_vcx[slot] + dock_slide.get(slot).copied().unwrap_or(0.0) * dock_slot;
        let baseline = slot_rect.y + slot_rect.h - DOCK_BASELINE_PAD * icon_scale;
        // Running indicator (macOS dots): one dot beneath the icon for each
        // of its windows (Max, 2026-10-16: "the running instances in dot
        // quantities"). Drawn before the icon so a magnified icon never
        // hides them.
        // (Four at most: four means four or more — Max, 2026-10-16.)
        let windows = dock_running.get(slot).copied().unwrap_or(0).min(4);
        // Beside a SIDE dock the icon stands on its slot's edge side (the
        // screen's edge, as a bottom dock's on its baseline) and grows away
        // from it; its dots down that side.
        let edge_x = match side {
            Side::Left => slot_rect.x + DOCK_BASELINE_PAD * icon_scale,
            _ => slot_rect.x + slot_rect.w - DOCK_BASELINE_PAD * icon_scale,
        };
        if sideways {
            let dot_x = if side == Side::Left { edge_x + DOCK_DOT_GAP } else { edge_x - DOCK_DOT_GAP };
            dock_dots_col(&mut scene.rects, dot_x, vcx, windows, dock_icon * 0.8);
        } else {
            dock_dots(&mut scene.rects, vcx, baseline + DOCK_DOT_R - DOCK_DOT_GAP, windows, dock_icon * 0.8);
        }
        let scale = dock_scales[slot];
        let across = if sideways { slot_rect.w } else { slot_rect.h };
        let size = (dock_icon * scale).min(across.min(dock_slot) + MAGNIFY_HEADROOM) * shown;
        // AGUA: the icon elongates/compresses vertically about its
        // baseline with the card's motion — a water drop on the bar
        // (amplified: at icon scale the raw factor reads as nothing).
        let drop = 1.0 + (stretch - 1.0) * DOCK_STRETCH_GAIN;
        let icon_rect = if sideways {
            let x = if side == Side::Left { edge_x + dock_lift(entry_idx) } else { edge_x - size - dock_lift(entry_idx) };
            Rect::new(x, vcx - size / 2.0, size, size)
        } else {
            Rect::new(
                vcx - size / 2.0,
                baseline - (size + dock_lift(entry_idx)) * drop,
                size,
                size * drop,
            )
        };
        // A drag hovering this icon's center (a fold target) rings it.
        if drag.and_then(|d| d.over_dock) == Some(slot) {
            let hl = dock_highlight;
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
                border: 0.0,
            });
        }
        // A minimized-window tile: the window's live thumbnail drawn at the
        // window's own shape (the square texture un-stretches into the aspect
        // rect layout() sized this slot to), with the app's icon badged into
        // the bottom-right corner (macOS). A tile whose thumbnail failed to
        // load falls through to the letter-tile path below. Recovering the
        // tile width from the slot (rather than re-reading the aspect) keeps
        // draw and hit-test on the one shape.
        let is_min = slot >= layout.dock_min_start;
        if is_min && !placeholders.get(entry_idx).copied().unwrap_or(false) {
            // Fit into the same box `layout()` sized the slot to: full height
            // (× MIN_TILE_SCALE) for portrait/near-square windows, but a WIDE
            // window is width-capped and its HEIGHT shrinks so it stays a
            // small tile. Vertically CENTERED in the icon band.
            let aspect = min_aspects.get(slot - layout.dock_min_start).copied().unwrap_or(1.0);
            let full_h = size * MIN_TILE_SCALE; // magnifies with `size`
            let (tile_w, tile_h) = min_tile_dims(aspect, full_h);
            let thumb = if sideways {
                // Its square slot: a wide window fitted to its width.
                let k = (size / tile_w.max(1.0)).min(1.0);
                let (tw, th) = (tile_w * k, tile_h * k);
                let (cx, cy) = (icon_rect.x + icon_rect.w / 2.0, icon_rect.y + icon_rect.h / 2.0);
                Rect::new(cx - tw / 2.0, cy - th / 2.0, tw, th)
            } else {
                let icon_cy = baseline - (size + dock_lift(entry_idx)) * drop / 2.0; // the pinned icon's vertical mid
                Rect::new(vcx - tile_w / 2.0, icon_cy - tile_h * drop / 2.0, tile_w, tile_h * drop)
            };
            // The thumbnail fills the rect UNPLATED: it is a picture, not a
            // glyph, so it keeps its true rectangular corners (the shader
            // skips the squircle for `layer >= thumb_base`).
            scene.icons.push(IconInst {
                rect: thumb,
                layer: layer_of(entry_idx),
                tint: [0.0; 4],
                ring: -1.0,
                plate: NO_PLATE,
            });
            // App-icon badge, bottom-right — a normal app icon, so it wears
            // the squircle + adaptive plate. Skipped when the window's class
            // matched no app (no placeholder badge, per the design).
            if let Some(Some(badge_layer)) = min_badges.get(slot - layout.dock_min_start).copied() {
                let bs = tile_h * MIN_BADGE_FRAC;
                let inset = MIN_BADGE_INSET * icon_scale;
                scene.icons.push(IconInst {
                    rect: Rect::new(
                        thumb.x + thumb.w - bs - inset,
                        thumb.y + thumb.h - bs - inset,
                        bs,
                        bs,
                    ),
                    layer: badge_layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate,
                });
            }
            continue;
        }
        // The Recycle Bin: the same rounded "squircle" tile shape the
        // box/folder cells use (rect-shader `radius` + glass), wearing the
        // live adaptive plate colour like every app icon's plate — warming
        // to red while an app is dragged, with a can on top.
        if entries
            .get(entry_idx)
            .is_some_and(|e| crate::groups::is_trash(&e.id))
        {
            // Its states (`bin_motion`): orange while something is in hand,
            // red and swollen over it, a rim while a file window is in it.
            let rect = grow_from_edge(icon_rect, bin.swell, side);
            // (The bin's tile is its plate: it fades with the dock's icons.)
            let mut tile = bin_tile_color(plate, bin.orange, bin.red);
            tile[3] *= crate::options::dock_icons();
            scene.rects.push(RectInst {
                rect,
                radius: rect.h * 0.22,
                color: tile,
                glass: 0.5,
                border: 0.0,
            });
            // The same subtle rim the icon plates wear (see `plate_rim`).
            scene.rects.push(RectInst {
                rect,
                radius: rect.h * 0.22,
                color: plate_rim(tile),
                glass: 0.0,
                border: 1.0,
            });
            if bin.open > 0.0 {
                scene.rects.push(RectInst {
                    rect,
                    radius: rect.h * 0.22,
                    color: [1.0, 1.0, 1.0, 0.35 * bin.open * crate::options::dock_icons()],
                    glass: 0.0,
                    border: 2.0,
                });
            }
            // The metal bin figure (the tile behind it is the plate).
            push_bin_posed(&mut scene, rect, layer_of(entry_idx), &bin);
            continue;
        }
        if let Some((_, minis)) = group_minis.iter().find(|(e, _)| *e == entry_idx) {
            // A pinned box: a folder tile with a 2×2 mini preview.
            let hl = dock_highlight;
            scene.rects.push(RectInst {
                rect: icon_rect,
                radius: size * 0.22,
                // (A tile of the dock's: it fades with the dock's icons.)
                color: [hl[0], hl[1], hl[2], (hl[3] * 4.0).min(0.72) * crate::options::dock_icons()],
                glass: 0.5,
                border: 0.0,
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
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate,
                });
            }
            continue;
        }
        // Golem Files' launcher: the plate holds still on a blank layer, the
        // folders over it come out from behind the front one on hover.
        if let Some((first, out)) =
            files_fig.filter(|_| entries.get(entry_idx).is_some_and(|e| e.id == crate::filer::APP_ID))
        {
            scene.icons.push(IconInst {
                rect: icon_rect,
                layer: first + crate::files_icon::BLANK,
                tint: [0.0; 4],
                ring: -1.0,
                plate: plate_of(entry_idx),
            });
            for (layer, rect) in crate::files_icon::quads(icon_rect, out, files_wave) {
                scene.icons.push(IconInst { rect, layer: first + layer, tint: [0.0; 4], ring: -1.0, plate: NO_PLATE });
            }
            continue;
        }
        // THE APPS BUTTON: while it holds pins it and its plate grow, the
        // plate cut by its squares (the stencil) so the pin behind shows
        // through; its beat grows the figure (its plate holding still).
        if entries.get(entry_idx).is_some_and(|e| e.id == crate::apps::APPS_GRID_ID) {
            let held = overflow.map_or(0.0, |o| (o.max_off - o.off).clamp(0.0, 1.0));
            let about = |r: Rect, k: f32| {
                let (w, h) = (r.w * k, r.h * k);
                Rect::new(r.x + (r.w - w) / 2.0, r.y + (r.h - h) / 2.0, w, h)
            };
            let tile = about(icon_rect, 1.0 + crate::apps_icon::HOLDING_GROW * held);
            let figure = about(tile, apps_beat);
            let mut quads = Vec::new();
            match (apps_stencil.filter(|_| held > 0.01), files_fig) {
                (Some(stencil), _) => {
                    quads.push(IconInst { rect: tile, layer: stencil, tint: [0.0; 4], ring: -1.45, plate: plate_of(entry_idx) });
                    // (The figure faded too, a ghost: "todo más transparente".)
                    quads.push(IconInst { rect: figure, layer: layer_of(entry_idx), tint: [0.0; 4], ring: -1.18, plate: [0.0, 0.0, 0.0, 0.75] });
                }
                (None, Some((first, _))) if apps_beat != 1.0 => {
                    quads.push(IconInst { rect: tile, layer: first + crate::files_icon::BLANK, tint: [0.0; 4], ring: -1.0, plate: plate_of(entry_idx) });
                    quads.push(IconInst { rect: figure, layer: layer_of(entry_idx), tint: [0.0; 4], ring: -1.0, plate: NO_PLATE });
                }
                _ => quads.push(IconInst { rect: tile, layer: layer_of(entry_idx), tint: [0.0; 4], ring: -1.0, plate: plate_of(entry_idx) }),
            }
            if overflow.is_some() {
                apps_top.extend(quads);
            } else {
                scene.icons.extend(quads);
            }
            apps_drawn = Some(tile);
            continue;
        }
        scene.icons.push(IconInst {
            rect: icon_rect,
            layer: layer_of(entry_idx),
            tint: [0.0; 4],
            ring: -1.0,
            plate: plate_of(entry_idx),
        });
        // A package installing straight onto the dock (drag-to-dock pins the
        // tile there) animates the same macOS-style progress ring the grid
        // tiles show, over its pinned slot — drawn on top of the icon.
        let dock_prog = progress.get(entry_idx).copied().unwrap_or(-1.0);
        if dock_prog >= 0.0 {
            scene.icons.push(IconInst {
                rect: icon_rect,
                layer: 0,
                tint: [0.0; 4],
                ring: dock_prog.clamp(0.0, 1.0),
                plate: NO_PLATE,
            });
        }
        // A just-installed app surfaced here plays a one-shot glass shine over
        // its dock icon (`ring = 2.0 + progress`; the shader clips it to the
        // icon silhouette). This covers both a drag-to-dock install's
        // completion sweep and a hidden-completion notify's one-shot shine.
        let dock_shine = shine.get(entry_idx).copied().unwrap_or(-1.0);
        if dock_shine >= 0.0 {
            scene.icons.push(IconInst {
                rect: icon_rect,
                layer: layer_of(entry_idx),
                tint: [0.0; 4],
                ring: 2.0 + dock_shine.clamp(0.0, 1.0),
                plate: NO_PLATE,
            });
        }
        if placeholders.get(entry_idx).copied().unwrap_or(false) {
            if let Some(ch) = entries.get(entry_idx).and_then(|e| e.name.chars().next()) {
                let letter: String = ch.to_uppercase().collect();
                let font_px = (size * 0.46).clamp(10.0, 28.0);
                let line_px = font_px * 1.3;
                let (letter_x, icon_center_y) = if sideways {
                    (icon_rect.x + icon_rect.w / 2.0, icon_rect.y + icon_rect.h / 2.0)
                } else {
                    (vcx, baseline - size / 2.0 - dock_lift(entry_idx))
                };
                scene.labels.push(Label {
                    text: letter,
                    pos: (letter_x, icon_center_y - line_px / 2.0),
                    max_w: size,
                    font_px,
                    line_px,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: None,
                    clip: None,
                });
            }
        }
    }
    // The overflow's line ends: the pins half out at the row's left edge and
    // those going in behind the apps button, full size, a slot apart, cut at
    // the row's start and at the button's right side (short of its round
    // corner, so nothing shows past it); then the button over them. (The
    // count pill this replaced is gone: the last one in shows behind.)
    if let (Some(ov), Some(apps)) = (overflow.filter(|_| sideways), apps_drawn) {
        // DOWN A SIDE DOCK: the same line along the column, cut at its top
        // and just past the button's lower side, the button over them.
        let apps_cy = dock_vcx.get(ov.apps_slot).copied().unwrap_or(apps.y + apps.h / 2.0);
        let top = layout.dock_slots.first().map_or(0.0, |r| r.y);
        let mut edge = GridContent {
            clip: Rect::new(0.0, top, surface.0, (apps.y + apps.h * 0.78) - top),
            ..Default::default()
        };
        for &(slot, entry_idx, s) in &edge_pins {
            let Some(slot_rect) = layout.dock_slots.get(slot) else { continue };
            let scale = dock_scales.get(slot).copied().unwrap_or(1.0);
            let size = (dock_icon * scale).min(slot_rect.w.min(dock_slot) + MAGNIFY_HEADROOM);
            let own = dock_slot + dock_icon * (scale - 1.0);
            let step = (dock_vw.get(ov.apps_slot).copied().unwrap_or(dock_slot) + own) / 2.0;
            let cy = if s < 0.0 {
                dock_vcx.get(slot + 1).map_or(apps_cy, |y| y - own)
            } else {
                apps_cy + (s - ov.cap as f32) * step
            };
            let x = match side {
                Side::Left => slot_rect.x + DOCK_BASELINE_PAD * icon_scale + dock_hop,
                _ => slot_rect.x + slot_rect.w - DOCK_BASELINE_PAD * icon_scale - size - dock_hop,
            };
            edge.icons.push(IconInst {
                rect: Rect::new(x, cy - size / 2.0, size, size),
                layer: layer_of(entry_idx),
                tint: [0.0; 4],
                ring: -1.0,
                plate: plate_of(entry_idx),
            });
        }
        if let Some(slot_rect) = layout.dock_slots.get(ov.apps_slot) {
            let edge_x = match side {
                Side::Left => slot_rect.x + DOCK_BASELINE_PAD * icon_scale + DOCK_DOT_GAP,
                _ => slot_rect.x + slot_rect.w - DOCK_BASELINE_PAD * icon_scale - DOCK_DOT_GAP,
            };
            dock_dots_col(&mut scene.rects, edge_x, apps_cy, ov.hidden_right, apps.h * 0.8);
        }
        // (The edge pins and the Apps button are dock icons too.)
        edge.fade = Some(crate::options::dock_icons());
        scene.grids.push(edge);
        scene.grids.push(GridContent {
            clip: Rect::new(0.0, 0.0, surface.0, surface.1),
            icons: std::mem::take(&mut apps_top),
            fade: Some(crate::options::dock_icons()),
            ..Default::default()
        });
    } else if let (Some(ov), Some(apps)) = (overflow, apps_drawn) {
        let apps_cx = dock_vcx.get(ov.apps_slot).copied().unwrap_or(apps.x + apps.w / 2.0);
        let mut edge = GridContent {
            clip: Rect::new(
                layout.dock_slots.first().map_or(0.0, |r| r.x),
                0.0,
                (apps.x + apps.w * 0.78) - layout.dock_slots.first().map_or(0.0, |r| r.x),
                surface.1,
            ),
            ..Default::default()
        };
        let drop = 1.0 + (stretch - 1.0) * DOCK_STRETCH_GAIN;
        for &(slot, entry_idx, s) in &edge_pins {
            let Some(slot_rect) = layout.dock_slots.get(slot) else { continue };
            // Its size and place as the row will have them once it is out —
            // magnified as its neighbours are under the pointer — so the
            // hand-over to its own slot is seamless (it came out small and
            // then jumped up to size, Max 2026-10-16).
            let scale = dock_scales.get(slot).copied().unwrap_or(1.0);
            let size = (dock_icon * scale).min(slot_rect.h.min(dock_slot) + MAGNIFY_HEADROOM);
            let own_vw = dock_slot + dock_icon * (scale - 1.0);
            let step = (dock_vw.get(ov.apps_slot).copied().unwrap_or(dock_slot) + own_vw) / 2.0;
            let cx = if s < 0.0 {
                dock_vcx.get(slot + 1).map_or(apps_cx, |x| x - own_vw)
            } else {
                // (cap - 1 → its own slot; cap → behind the button's middle.)
                apps_cx + (s - ov.cap as f32) * step
            };
            let baseline = slot_rect.y + slot_rect.h - DOCK_BASELINE_PAD * icon_scale - dock_hop;
            edge.icons.push(IconInst {
                rect: Rect::new(cx - size / 2.0, baseline - size * drop, size, size * drop),
                layer: layer_of(entry_idx),
                tint: [0.0; 4],
                ring: -1.0,
                plate: plate_of(entry_idx),
            });
        }
        // A dot under the button for each pin it holds, as the running
        // apps have theirs (Max, 2026-10-16): they go out one by one as it
        // spits; many draw closer so the row stays under the button.
        if let Some(slot_rect) = layout.dock_slots.get(ov.apps_slot) {
            let baseline = slot_rect.y + slot_rect.h - DOCK_BASELINE_PAD * icon_scale;
            dock_dots(&mut scene.rects, apps_cx, baseline + DOCK_DOT_R - DOCK_DOT_GAP, ov.hidden_right, apps.w * 0.8);
        }
        // (The edge pins and the Apps button are dock icons too.)
        edge.fade = Some(crate::options::dock_icons());
        scene.grids.push(edge);
        scene.grids.push(GridContent {
            clip: Rect::new(0.0, 0.0, surface.0, surface.1),
            icons: std::mem::take(&mut apps_top),
            fade: Some(crate::options::dock_icons()),
            ..Default::default()
        });
    }
    // Hover tooltip: the hovered dock icon's app name in a pill above it
    // (dock icons carry no label of their own, unlike grid cells). Gated
    // behind a hover dwell by the caller (`dock_tooltip`).
    if drag.is_none() {
        // Not under a box open over the dock: its rows stand where the pill would.
        if let Some(i) = dock_tooltip.filter(|_| !box_over_dock) {
            if let (Some(&entry_idx), Some(slot), Some(&vcx)) =
                (dock_order.get(i), layout.dock_slots.get(i), dock_vcx.get(i))
            {
                if let Some(name) = entries.get(entry_idx).map(|e| e.name.clone()) {
                    let font_px = LABEL_FONT_PX;
                    let line_px = font_px * 1.3;
                    let scale = dock_scales.get(i).copied().unwrap_or(1.0);
                    let size = (dock_icon * scale).min(slot.h.min(dock_slot) + MAGNIFY_HEADROOM);
                    let icon_top = slot.y + slot.h - size - dock_lift(entry_idx);
                    // Width from the same glyph-advance model truncate_label uses.
                    let pill_w = name.chars().count() as f32 * font_px * 0.52 + 16.0;
                    let pill_h = line_px + 6.0;
                    // Above a bottom dock's icon; beside a side dock's, on
                    // the screen's side of it.
                    let (pill_cx, pill_y) = match side {
                        Side::Bottom => (vcx, (icon_top - 6.0 - pill_h).max(2.0)),
                        Side::Left => (slot.x + DOCK_BASELINE_PAD * icon_scale + size + dock_lift(entry_idx) + 6.0 + pill_w / 2.0, vcx - pill_h / 2.0),
                        Side::Right => (slot.x + slot.w - DOCK_BASELINE_PAD * icon_scale - size - dock_lift(entry_idx) - 6.0 - pill_w / 2.0, vcx - pill_h / 2.0),
                    };
                    let mut bg = dock_bg;
                    bg[3] = crate::options::shell_glass(bg[3].max(0.92));
                    scene.rects.push(RectInst {
                        rect: Rect::new(pill_cx - pill_w / 2.0, pill_y, pill_w, pill_h),
                        radius: pill_h / 2.0,
                        color: bg,
                        glass: 0.0,
                        border: 0.0,
                    });
                    scene.labels.push(Label {
                        text: name,
                        pos: (pill_cx, pill_y + (pill_h - line_px) / 2.0),
                        max_w: pill_w,
                        font_px,
                        line_px,
                        centered: true,
                        dim: false,
                        cache: true,
                        family: None,
                        color: None,
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
            let hl = dock_highlight;
            scene.rects.push(RectInst {
                rect: Rect::new(vp.x - 6.0, vp.y - 4.0, vp.w + 12.0, vp.h + 8.0),
                radius: 14.0,
                color: [hl[0], hl[1], hl[2], (hl[3] * 1.4).min(0.3)],
                glass: 0.0,
                border: 0.0,
            });
        }
        // Over an uninstall target, flush the ghost red — the icon in hand is
        // the one about to disappear, so it reddens to say "drop = remove".
        // Full red over the Install section; eased in with the hover-grow while
        // it's over the Recycle Bin.
        // `ghost_ring` = -1.0 keeps the shader's composited trash-can mark (the
        // Install-section drop has no bin, so the mark is its only "= remove"
        // cue); -1.3 reddens the ghost but skips the can (over the Recycle Bin
        // the bin itself is the can — a second one would be redundant).
        let (tint, ghost_ring) = if df.drop_section == Some(SECTION_INSTALL) {
            ([0.52, 0.11, 0.09, 0.55], -1.0)
        } else {
            ([0.52, 0.11, 0.09, 0.55 * trash_hover], -1.3)
        };
        // Ghost following the pointer, topmost: the dragged icon, or a
        // box's mini stack. Sized to the row it hovers — dock icons over
        // the dock, grid cells over the grid — so the grabbed icon always
        // matches its neighbours, shrinking onto the dock on the fly.
        // Shrink the ghost a little while it hovers the bin — it reads as being
        // drawn into the trash the bin is swelling to accept.
        let size = if df.on_dock { dock_icon } else { grid_icon } * (1.0 - 0.22 * trash_hover);
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
                    tint,
                    ring: ghost_ring,
                    plate,
                });
            }
        } else {
            scene.overlay.push(IconInst {
                rect: Rect::new(gx - size / 2.0, gy - size / 2.0, size, size),
                layer: layer_of(df.entry_idx),
                tint,
                ring: ghost_ring,
                plate: plate_of(df.entry_idx),
            });
        }
    }

    // Search widget: "Filter" pill button that morphs into an expanding
    // search box. search_expand drives the open animation (0→1). Rides
    // the card with the rest of the content, clipped to the reveal band
    // so it sinks into the card's bottom edge while closing.
    // (Built here, pushed after the section grids so `scene.grids[s]`
    // keeps indexing the sections.)
    // Apps ↔ settings is a push (see `FrameInput::panel_mix`): everything
    // the apps content adds from here on is shifted up together below, so
    // note where it starts.
    let apps_from = (scene.labels.len(), scene.rects.len(), scene.grids.len());
    let mut search_grid: Option<GridContent> = None;
    // The search pill belongs to both views: it stays through the push.
    // (Beside a side dock: once any of the box shows; the clip does the rest.)
    if (sideways && reveal_rect.w > 1.0)
        || (!sideways && layout.search_box.y >= layout.card_top + config.window.input_bar_height as f32 * icon_scale)
    {
        let mut sgrid = GridContent {
            clip: reveal_rect,
            ..Default::default()
        };
        let boxx = layout.search_box;
        let cx = boxx.x + boxx.w / 2.0;
        // One more pill: the control panel's middle layer (the OPTIONS bar
        // pill) — its wash, soft light and ink, its type, and its hover
        // (settles in, lifts 7%, the OPTIONS hover wash).
        let sp = search_pill;
        let bright = dock_ink[0] + dock_ink[1] + dock_ink[2] < 1.5;
        use crate::panel::{HOVER_WASH_BRIGHT, HOVER_WASH_DARK, LAYER_GLOW, LAYER_INK, LAYER_WASH_BRIGHT, LAYER_WASH_DARK, LIFT};
        let (rest_a, hover_a) = if bright {
            (LAYER_WASH_BRIGHT[1], HOVER_WASH_BRIGHT)
        } else {
            (LAYER_WASH_DARK[1], HOVER_WASH_DARK)
        };
        let box_color = crate::options::wash(!bright, lerp(rest_a, hover_a, sp.lift));
        let font = crate::options::FONT_PX * sp.scale;
        let line = crate::options::LINE_PX * sp.scale;
        let pad = crate::options::PILL_PAD_X * sp.scale;
        // Compact: hugs "Search" like any pill. Expanded: at least
        // SEARCH_W_MIN, growing snugly with the shaped query (measured, not
        // estimated) plus caret room.
        let compact_w = (sp.label_px + 2.0 * pad).max(sp.h);
        // (The deep search's chip stands before the words: "Files ›".)
        let chip_room = deep.map_or(0.0, |d| d.chip_w + 2.0 * pad * 0.6 + PILL_CHIP_GAP);
        let typed_px = match deep {
            Some(d) if query.is_empty() => d.placeholder_w,
            _ => query_px,
        };
        let content_w = (typed_px + chip_room + 2.0 * pad + 8.0)
            .max(SEARCH_W_MIN)
            .min(card_w - 2.0 * GRID_PAD_X);
        let k = 1.0 + LIFT * sp.lift;
        let (sw, sh) = (lerp(compact_w, content_w, search_expand) * k, sp.h * k);
        let cy = boxx.y + SEARCH_H / 2.0;
        let draw_rect = Rect::new(cx - sw / 2.0, cy - sh / 2.0, sw, sh);
        let (blur, glow_a) = LAYER_GLOW[1];
        let glow = if bright { 0.0 } else { 1.0 };
        scene.overlay_shadows.push(ShadowInst {
            rect: draw_rect,
            radius: sh / 2.0,
            blur: blur * sp.scale,
            color: [glow, glow, glow, glow_a * card_open * card_open],
            edges: [1.0, 1.0, 1.0, 1.0],
        });
        sgrid.rects.push(RectInst {
            rect: draw_rect,
            radius: sh / 2.0,
            color: box_color,
            glass: 0.0,
            border: 0.0,
        });
        let ink = faded_on(dock_ink, dock_seen, LAYER_INK[1], 4.5);
        let (text, dim) = if search_expand >= 0.5 && !query.is_empty() {
            (query.to_string(), false)
        } else if deep.is_some() {
            (crate::i18n::tr("Search files").to_string(), true)
        } else {
            // The label, then the placeholder once open.
            (crate::i18n::tr("Search").to_string(), search_expand >= 0.5)
        };
        // The deep search's chip, at the pill's start: the words after it,
        // centred in what it leaves.
        let text_cx = match deep.filter(|_| search_expand >= 0.5) {
            Some(d) => {
                let (cw, ch) = (d.chip_w + 2.0 * pad * 0.6, (sh - 6.0 * sp.scale).max(line));
                let left = cx - (typed_px + chip_room) / 2.0;
                let chip = Rect::new(left, cy - ch / 2.0, cw, ch);
                sgrid.rects.push(RectInst {
                    rect: chip,
                    radius: ch / 2.0,
                    color: crate::options::wash(!bright, lerp(rest_a, hover_a, 1.0) * 1.6),
                    glass: 0.0,
                    border: 0.0,
                });
                sgrid.labels.push(Label {
                    text: d.chip.to_string(),
                    pos: (chip.x + cw / 2.0, cy - line * k / 2.0),
                    max_w: cw,
                    font_px: font * k,
                    line_px: line * k,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: crate::options::TEXT_FONT,
                    color: Some(ink),
                    clip: Some(reveal_clip(draw_rect)),
                });
                left + chip_room + typed_px / 2.0
            }
            None => cx,
        };
        sgrid.labels.push(Label {
            text,
            pos: (text_cx, cy - line * k / 2.0),
            max_w: sw - 2.0 * pad,
            font_px: font * k,
            line_px: line * k,
            centered: true,
            dim: false,
            // Glyphs cache only at rest (an easing size would fill it).
            cache: (query.is_empty() || search_expand < 0.5) && (k - 1.0).abs() < 0.001,
            family: crate::options::TEXT_FONT,
            // (The placeholder: the renderer's .45 dim, laid solid.)
            color: Some(if dim { faded_on(dock_ink, dock_seen, 0.45, 4.5) } else { ink }),
            clip: Some(reveal_clip(draw_rect)),
        });
        // A real caret sitting exactly after the shaped text, only once
        // the morph has (nearly) landed, so it can't drift during the
        // stretch.
        if search_expand >= 0.5 && (!query.is_empty() || deep.is_some()) && search_expand > 0.9 {
            let caret_x = if deep.is_some() && query.is_empty() {
                text_cx - typed_px / 2.0 - 1.0
            } else {
                text_cx + query_px / 2.0 + 3.0
            }
            .min(draw_rect.x + sw - 8.0);
            let t = dock_ink;
            sgrid.rects.push(RectInst {
                rect: Rect::new(caret_x, cy - line / 2.0 + 1.0, 1.5, line - 2.0),
                radius: 0.75,
                color: [t[0], t[1], t[2], 0.9],
                glass: 0.0,
                border: 0.0,
            });
        }
        search_grid = Some(sgrid);
    }

    // The section titles' ink.
    let title_ink = faded_on(dock_ink, dock_seen, TITLE_ALPHA, 3.0);
    // The three sections: title, grid cells with per-section horizontal
    // paging, page dots, and per-section empty states.
    for (s, sec) in layout.sections.iter().enumerate() {
        // The panel clears the sections away; an empty grid per section
        // keeps `scene.grids[s]` indexing them.
        if panel {
            scene.grids.push(GridContent {
                clip: reveal_clip(sec.viewport),
                ..Default::default()
            });
            continue;
        }
        // (A deep search's hidden shelf: nothing at all.)
        if deep.is_some_and(|d| d.hidden[s]) {
            scene.grids.push(GridContent { clip: reveal_clip(sec.viewport), ..Default::default() });
            continue;
        }
        let title = if let Some(d) = deep {
            d.titles[s].to_string()
        } else if s == SECTION_APPS && !apps_group.is_empty() {
            format!("{} — {}", crate::i18n::tr(SECTION_TITLES[s]), apps_group)
        } else {
            crate::i18n::tr(SECTION_TITLES[s]).to_string()
        };
        // A box open over the top row stands on the title's line.
        let title_y = sec.title_pos.1 + TITLE_DROP;
        let under_box = open_box_rect.is_some_and(|b| b.y < title_y + TITLE_LINE_PX && b.y + b.h > title_y);
        scene.labels.push(Label {
            text: if under_box { String::new() } else { title },
            pos: (sec.title_pos.0, title_y),
            max_w: sec.viewport.w,
            font_px: TITLE_FONT_PX,
            line_px: TITLE_LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            family: Some(FONT_BOLD),
            color: Some(title_ink),
            clip: Some(reveal_rect),
        });
        // The lone app's recents' title (`app_actions.rs`), in the room
        // opened under its row: below the second lines of its actions' names.
        if let Some(text) = recent_title.filter(|_| s == SECTION_APPS) {
            let room = crate::app_actions::RECENT_ROOM * icon_scale;
            let y = sec.viewport.y + grid_cell_h + LABEL_LINE_PX + (room - LABEL_LINE_PX - TITLE_LINE_PX) / 2.0;
            scene.labels.push(Label {
                text: text.to_string(),
                pos: (sec.title_pos.0, y),
                max_w: sec.viewport.w,
                font_px: TITLE_FONT_PX,
                line_px: TITLE_LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: Some(FONT_BOLD),
                color: Some(title_ink),
                clip: Some(reveal_rect),
            });
        }

        let mut grid = GridContent {
            clip: reveal_clip(sec.viewport),
            ..Default::default()
        };
        if sec.cells == 0 && s == SECTION_APPS && layout.apps_none {
            // A search that left no app: "No results" under the title, as
            // the line makes room for it (`files_take_apps_row`).
            let line = LABEL_LINE_PX + 2.0;
            let mut color = faded_on(dock_ink, dock_seen, 0.45, 4.5);
            color[3] *= (sec.viewport.h / NO_APPS_H).clamp(0.0, 1.0);
            grid.labels.push(Label {
                text: crate::i18n::tr("No results").to_string(),
                pos: (sec.viewport.x + sec.viewport.w / 2.0, sec.viewport.y + NO_APPS_H - line),
                max_w: 200.0,
                font_px: LABEL_FONT_PX + 2.0,
                line_px: line,
                centered: true,
                dim: false,
                cache: true,
                family: None,
                color: Some(color),
                clip: Some(reveal_rect),
            });
            scene.grids.push(grid);
            continue;
        }
        if sec.cells == 0 {
            // Empty state: Install shows its index/search hint;
            // Apps/Files show one only when a search matched nothing.
            let text = match s {
                SECTION_INSTALL if !install_hint.is_empty() && deep.is_none() => install_hint,
                _ if !query.is_empty() || deep.is_some() => crate::i18n::tr("No results"),
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
                // (A side dock's one-column section: within its column.)
                max_w: if sec.vertical { sec.viewport.w + 2.0 * SECTION_GAP } else { 200.0 },
                font_px: LABEL_FONT_PX + 2.0,
                line_px: LABEL_LINE_PX + 2.0,
                centered: true,
                dim: false,
                cache: true,
                family: None,
                // (The empty state: the .45 dim, laid solid.)
                color: Some(faded_on(dock_ink, dock_seen, 0.45, 4.5)),
                clip: Some(reveal_rect),
            });
            scene.grids.push(grid);
            continue;
        }

        // (A page's length along the way the pages go: across, or down
        // beside a side dock.)
        let page_w = sec.page_len();
        // The section's own cell (narrower beside a side dock).
        let sec_cell_w = sec.viewport.w / sec.cols.max(1) as f32;
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
                // Apps rows part around a box open in the grid.
                let part = layout.apps_part.filter(|_| s == SECTION_APPS).map_or(0.0, |p| p.shift(row));
                if sec.vertical {
                    return (
                        sec.viewport.x + col as f32 * sec_cell_w + sec.row_shift(i),
                        sec.viewport.y + rel + row as f32 * grid_cell_h + part,
                    );
                }
                (
                    sec.viewport.x + rel + col as f32 * sec_cell_w + sec.row_shift(i),
                    sec.viewport.y + row as f32 * grid_cell_h + part,
                )
            };
            let (x0, y0) = corner(i0);
            if frac <= f32::EPSILON {
                return (x0, y0);
            }
            let (x1, y1) = corner(i0 + 1);
            (lerp(x0, x1, frac), lerp(y0, y1, frac))
        };
        // The slots taken, to know whose name has room for a second line.
        let display = |i: usize| if s == SECTION_APPS { apps_slide.get(i).copied().unwrap_or(i as f32) } else { i as f32 };
        let taken: std::collections::HashSet<usize> =
            (0..sec.cells.min(visible[s].len())).map(|i| display(i).round().max(0.0) as usize).collect();
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
            let g = &mut grid;
            let di = if s == SECTION_APPS {
                apps_slide.get(i).copied().unwrap_or(i as f32)
            } else {
                i as f32
            };
            let (cell_x, cell_y) = cell_pos(di);
            // (A lone app's recents slide between their pages.)
            let recent = s == SECTION_APPS && recents.is_some_and(|r| r.cells.contains(&entry_idx));
            let cell_x = cell_x + recents.filter(|_| recent).map_or(0.0, |r| r.slide * sec.viewport.w);
            let off_page = if sec.vertical {
                cell_y - sec.viewport.y <= -grid_cell_h || cell_y - sec.viewport.y >= page_w
            } else {
                cell_x - sec.viewport.x <= -sec_cell_w || cell_x - sec.viewport.x >= page_w
            };
            if off_page {
                continue;
            }
            let cell = Rect::new(cell_x, cell_y, sec_cell_w, grid_cell_h);
            if selected == Some((s, i)) {
                let hl = dock_highlight;
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                    radius: 14.0,
                    color: if selected_soft { hl } else { [hl[0], hl[1], hl[2], (hl[3] * 1.8).min(0.4)] },
                    glass: 0.0,
                    border: 0.0,
                });
            } else if selected.is_none() && hover == Some(Hit::GridCell(s, i)) {
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                    radius: 14.0,
                    color: dock_highlight,
                    glass: 0.0,
                    border: 0.0,
                });
            }
            // A drag hovering a cell that would take a drop
            // (group create/add) rings it brightly.
            if drag.and_then(|d| d.over_cell) == Some((s, i)) {
                let hl = dock_highlight;
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0),
                    radius: 16.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 2.2).min(0.5)],
                    glass: 0.0,
                    border: 0.0,
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
                    && ly + LABEL_FONT_PX > b.y
                    && ly < b.y + b.h
            });
            // The Recycle Bin, if it's been moved onto the grid: the same
            // rounded box-style tile in the live adaptive plate colour + a
            // can on top (plate → red while dragging).
            if crate::groups::is_trash(&entry.id) {
                let tile = box_tile;
                let rect = Rect::new(cx - tile / 2.0, icon_cy - tile / 2.0, tile, tile);
                let fill = trash_tile_color(plate, trash_react);
                g.rects.push(RectInst {
                    rect,
                    radius: 14.0,
                    color: fill,
                    glass: 0.5,
                    border: 0.0,
                });
                // The same subtle rim the icon plates wear (see `plate_rim`).
                g.rects.push(RectInst {
                    rect,
                    radius: 14.0,
                    color: plate_rim(fill),
                    glass: 0.0,
                    border: 1.0,
                });
                push_bin(&mut g.icons, rect, layer_of(entry_idx), trash_react);
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
                        family: None,
                        color: None,
                        clip: None,
                    });
                }
                continue;
            }
            if let Some((_, minis)) = group_minis.iter().find(|(e, _)| *e == entry_idx) {
                // Group cell: folder-style tile with a 2×2 mini
                // preview of the first members.
                let hl = dock_highlight;
                let tile = box_tile;
                g.rects.push(RectInst {
                    rect: Rect::new(cx - tile / 2.0, icon_cy - tile / 2.0, tile, tile),
                    radius: 14.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 4.0).min(0.72)],
                    glass: 0.5,
                    border: 0.0,
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
                        plate,
                        tint: [0.0; 4],
                        ring: -1.0,
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
                        family: None,
                        color: None,
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
            // THE LONE APP'S ACTIONS (`app_actions.rs`; Max's "3a"): the app's
            // own tile with its icon at half its light, the action's symbol
            // white over it with a soft shadow, the name on up to two lines.
            // The one asking to be sure wears a red plate.
            if let Some(&(_, sym)) = act_cells.iter().find(|(e, _)| *e == entry_idx) {
                let rect = Rect::new(cx - size / 2.0, icon_cy - size / 2.0 - lift(entry_idx), size, size);
                let dim = crate::app_actions::DIM;
                g.icons.push(IconInst {
                    rect,
                    layer: layer_of(entry_idx),
                    tint: [0.0, 0.0, 0.0, 1.0 - dim],
                    // (Below -1.15: no bin can over the darkened icon.)
                    ring: -1.3,
                    plate: if act_asking == Some(entry_idx) { asking_plate } else { plate_of(entry_idx) },
                });
                let s = size * crate::app_actions::SYMBOL;
                let at = |dy: f32, grow: f32| {
                    let w = s * grow;
                    Rect::new(rect.x + (size - w) / 2.0, rect.y + (size - w) / 2.0 + dy, w, w)
                };
                // The shadow: the symbol in black at a little under half, a
                // hair larger and lower; then the symbol itself (the ghost
                // mode: its own alpha in `plate.a`, no plate).
                g.icons.push(IconInst {
                    rect: at(size * 0.02, 1.06),
                    layer: sym,
                    tint: [0.0, 0.0, 0.0, 1.0],
                    ring: -1.18,
                    plate: [0.0, 0.0, 0.0, 0.45],
                });
                g.icons.push(IconInst {
                    rect: at(0.0, 1.0),
                    layer: sym,
                    tint: [1.0, 1.0, 1.0, 1.0],
                    ring: -1.18,
                    plate: [0.0, 0.0, 0.0, 1.0],
                });
                if !covered {
                    // (Its second line hangs a line below the grid's rows.)
                    let v = sec.viewport;
                    let clip = reveal_clip(Rect::new(v.x, v.y, v.w, v.h + 2.0 * LABEL_LINE_PX));
                    let spread = if act_wide == Some(entry_idx) { crate::app_actions::WIDE_LABEL } else { 1.0 };
                    for (k, line) in entry.name.lines().take(2).enumerate() {
                        g.labels.push(Label {
                            text: line.to_string(),
                            pos: (cx, cell.y + grid_icon_top + grid_icon + GRID_LABEL_GAP + k as f32 * LABEL_LINE_PX),
                            max_w: cell.w * spread - 12.0,
                            font_px: LABEL_FONT_PX,
                            line_px: LABEL_LINE_PX,
                            centered: true,
                            dim: false,
                            cache: true,
                            family: None,
                            color: None,
                            clip: Some(clip),
                        });
                    }
                }
                continue;
            }
            // A recent's preview: whole, in its own shape, its bottom on the icons'
            // line — and still under the pointer (Max: "the thumbnails move down on
            // hover, we don't need that"): no magnifying, it is a picture.
            let shape = recents.filter(|_| recent).and_then(|r| r.shapes.iter().find(|(e, _)| *e == entry_idx).map(|&(_, a)| a));
            g.icons.push(IconInst {
                rect: match shape {
                    Some(a) => preview_rect(cx, icon_cy + grid_icon / 2.0 - lift(entry_idx), a, cell.w, grid_icon),
                    None => Rect::new(cx - size / 2.0, icon_cy - size / 2.0 - lift(entry_idx), size, size),
                },
                layer: layer_of(entry_idx),
                tint: [0.0; 4],
                ring: -1.0,
                // Files rows are thumbnails and file-type carriers — the
                // rasterizer never plated those, so neither do we.
                plate: if s == SECTION_FILES {
                    NO_PLATE
                } else {
                    plate_of(entry_idx)
                },
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
                        family: None,
                        color: None,
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
            // A macOS-style circular progress ring over an installing tile:
            // pushed after the icon (so it draws on top), sharing its rect.
            // Once the ring fills, a glass-shine streak sweeps the icon (the
            // ring is gone by then — `prog` is negative during the shine).
            let prog = progress.get(entry_idx).copied().unwrap_or(-1.0);
            let shine_p = shine.get(entry_idx).copied().unwrap_or(-1.0);
            let icon_rect = Rect::new(
                cx - size / 2.0,
                icon_cy - size / 2.0 - lift(entry_idx),
                size,
                size,
            );
            if is_installing && prog >= 0.0 {
                g.icons.push(IconInst {
                    rect: icon_rect,
                    layer: 0,
                    tint: [0.0; 4],
                    ring: prog.clamp(0.0, 1.0),
                    plate: NO_PLATE,
                });
            } else if is_installing && shine_p >= 0.0 {
                // Shine instance: samples the app icon's silhouette (its layer)
                // to clip the streak, encoded as `ring = 2.0 + progress`.
                g.icons.push(IconInst {
                    rect: icon_rect,
                    layer: layer_of(entry_idx),
                    tint: [0.0; 4],
                    ring: 2.0 + shine_p.clamp(0.0, 1.0),
                    plate: NO_PLATE,
                });
            }
            let name = if is_busy && is_launching {
                crate::i18n::tr("Launching…").to_string()
            } else if is_busy && (s == SECTION_INSTALL || is_installing) {
                crate::i18n::tr("Installing…").to_string()
            } else if is_busy {
                crate::i18n::tr("Removing…").to_string()
            } else if is_failed {
                crate::i18n::tr("Failed").to_string()
            } else {
                tile_label(entry, kinds.get(entry_idx), cell.w - 12.0, LABEL_FONT_PX)
            };
            // A name too long for one line: two, broken between words, where
            // the space under it is free — no cell below it, a last row with
            // no page dots under it, the row a box or a lone app parts —; else
            // one with "…" (the rows are a line apart).
            let fit = names.and_then(|n| n.get(&entry_idx)).filter(|_| !is_busy && !is_failed);
            let slot = di.round().max(0.0) as usize;
            let row = (slot % cells_per_page) / sec.cols.max(1);
            let parted = s == SECTION_APPS && layout.apps_part.is_some_and(|p| row as isize == p.row);
            let bottom = cell.y + grid_cell_h > sec.viewport.y + sec.viewport.h - 0.5;
            let dots = sec.n_pages > 1 || (s == SECTION_APPS && recent_pages.is_some());
            let room_under = parted || if bottom { !dots } else { !taken.contains(&(slot + sec.cols)) };
            // (A deep search's file: its name on one line and, under it, when
            // it was used — where nothing stands under it; the page dots stand
            // lower than a line's first letters.)
            let sub = deep.and_then(|d| d.subs.get(&entry_idx)).filter(|_| !is_busy && !is_failed);
            let (name, second) = match (sub, fit) {
                (Some(_), Some(fit)) => (fit.one.clone(), None),
                (Some(_), None) => (name, None),
                (None, Some(fit)) if !room_under => (fit.one.clone(), None),
                (None, Some(fit)) => (fit.two.0.clone(), Some(fit.two.1.clone())),
                (None, None) => (name, None),
            };
            let free_under = parted || if bottom { true } else { !taken.contains(&(slot + sec.cols)) };
            if let (Some(sub), true, false) = (sub, free_under, covered) {
                let v = sec.viewport;
                g.labels.push(Label {
                    text: sub.clone(),
                    pos: (cx, cell.y + grid_icon_top + grid_icon + GRID_LABEL_GAP + SECOND_LINE - 1.0),
                    max_w: cell.w - 12.0,
                    font_px: LABEL_FONT_PX - 1.5,
                    line_px: LABEL_LINE_PX - 2.0,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: Some(faded_on(dock_ink, dock_seen, 0.55, 3.0)),
                    clip: Some(reveal_clip(Rect::new(v.x, v.y, v.w, v.h + 2.0 * LABEL_LINE_PX))),
                });
            }
            // (Picked to act on together: a check at the picture's corner.)
            if deep.is_some_and(|d| d.marked.contains(&entry_idx)) {
                let hl = dock_highlight;
                g.rects.push(RectInst {
                    rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                    radius: 14.0,
                    color: [hl[0], hl[1], hl[2], (hl[3] * 1.4).min(0.32)],
                    glass: 0.0,
                    border: 0.0,
                });
                let r = 9.0;
                let (bx, by) = (cx + grid_icon * 0.42, cell.y + grid_icon_top + 2.0);
                g.rects.push(RectInst {
                    rect: Rect::new(bx - r, by - r + r, 2.0 * r, 2.0 * r),
                    radius: r,
                    color: crate::palette::accent_srgb(),
                    glass: 0.0,
                    border: 0.0,
                });
                g.labels.push(Label {
                    text: "✓".to_string(),
                    pos: (bx, by + r - 7.0),
                    max_w: 2.0 * r,
                    font_px: 12.0,
                    line_px: 14.0,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: Some([1.0, 1.0, 1.0, 1.0]),
                    clip: None,
                });
            }
            if let (Some(second), false) = (second, covered) {
                let v = sec.viewport;
                g.labels.push(Label {
                    text: second,
                    pos: (cx, cell.y + grid_icon_top + grid_icon + GRID_LABEL_GAP + SECOND_LINE),
                    max_w: cell.w - 12.0,
                    font_px: LABEL_FONT_PX,
                    line_px: LABEL_LINE_PX,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: None,
                    clip: Some(reveal_clip(Rect::new(v.x, v.y, v.w, v.h + 2.0 * LABEL_LINE_PX))),
                });
            }
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
                    family: None,
                    color: None,
                    // (A recent's name slides with it, inside the box.)
                    clip: recent.then(|| reveal_clip(sec.viewport)),
                });
            }
        }
        // The recents' page that went, sliding out the other way.
        if let Some(r) = recents.filter(|r| s == SECTION_APPS && !r.leaving.is_empty()) {
            let dx = (r.slide - r.dir) * sec.viewport.w;
            for l in &r.leaving {
                let (x, y) = cell_pos(l.slot as f32);
                let (cx, icon_cy) = (x + dx + sec_cell_w / 2.0, y + grid_icon_top + grid_icon / 2.0);
                grid.icons.push(IconInst {
                    rect: match l.aspect {
                        Some(a) => preview_rect(cx, icon_cy + grid_icon / 2.0, a, sec_cell_w, grid_icon),
                        None => Rect::new(cx - grid_icon / 2.0, icon_cy - grid_icon / 2.0, grid_icon, grid_icon),
                    },
                    layer: l.layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate: NO_PLATE,
                });
                grid.labels.push(Label {
                    text: truncate_label(&l.name, sec_cell_w - 12.0, LABEL_FONT_PX),
                    pos: (cx, y + grid_icon_top + grid_icon + GRID_LABEL_GAP),
                    max_w: sec_cell_w - 12.0,
                    font_px: LABEL_FONT_PX,
                    line_px: LABEL_LINE_PX,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: None,
                    clip: Some(reveal_clip(sec.viewport)),
                });
            }
        }
        scene.grids.push(grid);

        // Page indicator dots in the gap beneath the section — gated by
        // the reveal edge (they ride the same fixed layout as the grid,
        // so unclipped they would linger as a ghost once the box hides).
        // Pages going down (beside a side dock): the dots stand in a column
        // on the box's own side of the section (Max, 2026-10-21): at its
        // left when the box is on the left, at its right when it is on the
        // right — right by its own column, not out in the gap ("they are the
        // apps dots": as under a bottom section, just past its edge).
        if sec.vertical {
            // (The icons stand centred in their cells: the outer column's
            // edge is the cell's margin in from the section's.)
            let margin = ((sec_cell_w - grid_icon) / 2.0).max(0.0);
            let dot_x = if layout.side == Side::Left {
                sec.viewport.x + margin - SIDE_DOTS_FROM_ICONS * icon_scale
            } else {
                sec.viewport.x + sec.viewport.w - margin + SIDE_DOTS_FROM_ICONS * icon_scale
            };
            let dot_cy = sec.viewport.y + sec.viewport.h / 2.0;
            if sec.n_pages > 1 && reveal_rect.contains((dot_x, dot_cy)) {
                let dot_r = 3.0;
                let dot_spacing = 10.0;
                let total = sec.n_pages as f32 * dot_spacing - (dot_spacing - dot_r * 2.0);
                let page_frac = sec.scroll / page_w;
                let hl = dock_highlight;
                for p in 0..sec.n_pages {
                    let y = dot_cy - total / 2.0 + p as f32 * dot_spacing + dot_r;
                    let raw = (p as f32 - page_frac).abs();
                    let dist = raw.min(sec.n_pages as f32 - raw).min(1.0);
                    scene.rects.push(RectInst {
                        rect: Rect::new(dot_x - dot_r, y - dot_r, dot_r * 2.0, dot_r * 2.0),
                        radius: dot_r,
                        color: [hl[0], hl[1], hl[2], hl[3] * (1.0 - dist * 0.72)],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
            }
            continue;
        }
        // (Under the deep search's lines of when, a line lower: into the gap.)
        let under_subs = if deep.is_some() { LABEL_LINE_PX - 2.0 } else { 0.0 };
        let dot_y = sec.viewport.y + sec.viewport.h + DOT_ROOM / 2.0 - DOTS_RAISE + under_subs;
        // A lone app's recents' pages: their dots where the section's would
        // be, under their line (the section itself has one page).
        if let Some((page, n)) = recent_pages.filter(|_| s == SECTION_APPS && sec.n_pages <= 1) {
            if (reveal_top..=reveal_bottom).contains(&dot_y) {
                let (dot_r, dot_spacing) = (3.0, 10.0);
                let total_dots_w = n as f32 * dot_spacing - (dot_spacing - dot_r * 2.0);
                let dot_cx = sec.viewport.x + sec.viewport.w / 2.0;
                let hl = dock_highlight;
                for p in 0..n {
                    let x = dot_cx - total_dots_w / 2.0 + p as f32 * dot_spacing + dot_r;
                    let alpha = if p == page { 1.0 } else { 0.28 };
                    scene.rects.push(RectInst {
                        rect: Rect::new(x - dot_r, dot_y - dot_r, dot_r * 2.0, dot_r * 2.0),
                        radius: dot_r,
                        color: [hl[0], hl[1], hl[2], hl[3] * alpha],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
            }
        }
        if sec.n_pages > 1 && (reveal_top..=reveal_bottom).contains(&dot_y) {
            let dot_r = 3.0;
            let dot_spacing = 10.0;
            let total_dots_w = sec.n_pages as f32 * dot_spacing - (dot_spacing - dot_r * 2.0);
            let dot_cx = sec.viewport.x + sec.viewport.w / 2.0;
            let page_frac = sec.scroll / page_w;
            let hl = dock_highlight;
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
                    border: 0.0,
                });
            }
        }
    }
    // The Controls row: the settings the search matches, as the control
    // panel's near-layer pills under a section title. Apps content: it
    // rides the push with the rest.
    // THE DEEP SEARCH'S TABS: words under the field as the boxes' tabs are
    // (the panel's word choices — Max: "use the underline for the selected
    // one, as on the boxes, and the same font"): the text font, the ones lit
    // in the full ink and UNDERLINED, the others at 55 %; the page's dots
    // under them.
    if let (Some(d), Some(row), false) = (deep, layout.tabs_row, panel) {
        // The pages slide: each tab where its page stands now, round the end
        // the nearer way, inside the row.
        let (_, pages) = layout.tabs_page;
        let n = pages.max(1) as f32;
        let wrapped = |x: f32| {
            let r = x.rem_euclid(n);
            if r > n / 2.0 { r - n } else { r }
        };
        let in_row = reveal_clip(row);
        let mut tgrid = GridContent { clip: reveal_rect, ..Default::default() };
        for (i, rest) in layout.tabs_at.iter().enumerate() {
            let p = i / crate::deep_search::TABS_PAGE;
            let off = if pages > 1 { wrapped(p as f32 - layout.tabs_slide) } else { 0.0 };
            if off.abs() >= 1.0 || rest.w <= 0.0 {
                continue;
            }
            let Some(word) = d.tabs.get(i) else {
                continue;
            };
            let r = Rect::new(rest.x + off * row.w, rest.y, rest.w, rest.h);
            let s = layout.tabs_scales.get(p).copied().unwrap_or(1.0);
            let (font, line) = (crate::deep_search::TAB_FONT_PX * s, crate::deep_search::TAB_LINE_PX * s);
            let lit = d.lit.contains(&i);
            let a = if lit {
                1.0
            } else if hover == Some(Hit::Control(i)) {
                0.78
            } else {
                0.55
            };
            let ink = if a >= 1.0 { [dock_ink[0], dock_ink[1], dock_ink[2], dock_ink[3]] } else { faded_on(dock_ink, dock_seen, a, 2.5) };
            // (A little up from the row's middle: the dots under them.)
            let top = r.y + (r.h - line) / 2.0 - 9.0;
            tgrid.labels.push(Label {
                text: word.clone(),
                pos: (r.x + r.w / 2.0, top),
                max_w: r.w,
                font_px: font,
                line_px: line,
                centered: true,
                dim: false,
                cache: (s - 1.0).abs() < 0.001,
                family: crate::options::TEXT_FONT,
                color: Some(ink),
                clip: Some(in_row),
            });
            // The underline: the word's width, a hair under it (the panel's
            // 1.5 px at its big words).
            if lit {
                let uw = layout.tabs_words.get(i).copied().unwrap_or(r.w);
                let ux = r.x + (r.w - uw) / 2.0;
                let line_rect = Rect::new(ux, top + line + 1.0, uw, 1.5 * s.max(0.7));
                let u = line_rect.intersect(&in_row);
                if u.w > 0.0 && u.h > 0.0 {
                    tgrid.rects.push(RectInst { rect: u, radius: 0.0, color: ink, glass: 0.0, border: 0.0 });
                }
            }
        }
        if pages > 1 {
            let (dot_r, spacing) = (2.5, 9.0);
            let total = pages as f32 * spacing - (spacing - dot_r * 2.0);
            let (cx, cy) = (row.x + row.w / 2.0, row.y + row.h - 4.5);
            let hl = dock_highlight;
            for p in 0..pages {
                let x = cx - total / 2.0 + p as f32 * spacing + dot_r;
                // (Lit as the slide passes it, as the sections' dots are.)
                let dist = wrapped(p as f32 - layout.tabs_slide).abs().min(1.0);
                let on = 1.0 - dist * 0.72;
                tgrid.rects.push(RectInst {
                    rect: Rect::new(x - dot_r, cy - dot_r, dot_r * 2.0, dot_r * 2.0),
                    radius: dot_r,
                    color: [hl[0], hl[1], hl[2], hl[3] * on],
                    glass: 0.0,
                    border: 0.0,
                });
            }
        }
        scene.grids.push(tgrid);
    }
    if !layout.control_bands.is_empty() && !layout.controls.is_empty() && !panel && deep.is_none() {
        // One title over the one row: "System" — the settings and the
        // modules together (Max, 2026-10-22).
        if let Some(&(title, _)) = layout.control_bands.first() {
            scene.labels.push(Label {
                text: crate::i18n::tr("System").to_string(),
                pos: (title.0, title.1 + TITLE_DROP),
                max_w: 200.0,
                font_px: TITLE_FONT_PX,
                line_px: TITLE_LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: Some(FONT_BOLD),
                color: Some([title_ink[0], title_ink[1], title_ink[2], title_ink[3] * layout.controls_k]),
                clip: Some(reveal_rect),
            });
        }
        use crate::panel::{
            HOVER_WASH_BRIGHT, HOVER_WASH_DARK, LAYER_GLOW, LAYER_INK, LAYER_SCALE, LAYER_WASH_BRIGHT, LAYER_WASH_DARK, LIFT,
        };
        let bright = dock_ink[0] + dock_ink[1] + dock_ink[2] < 1.5;
        let (rest_a, hover_a) = if bright {
            (LAYER_WASH_BRIGHT[0], HOVER_WASH_BRIGHT)
        } else {
            (LAYER_WASH_DARK[0], HOVER_WASH_DARK)
        };
        let sc = search_pill.scale;
        let near = LAYER_SCALE[0];
        let (font, line) = (crate::options::FONT_PX * sc * near, crate::options::LINE_PX * sc * near);
        let ink = [dock_ink[0], dock_ink[1], dock_ink[2], dock_ink[3] * LAYER_INK[0]];
        let glow = if bright { 0.0 } else { 1.0 };
        let mut cgrid = GridContent {
            clip: reveal_rect,
            ..Default::default()
        };
        for (i, (r, &(label, _))) in layout.controls.iter().zip(controls).enumerate() {
            if r.w <= 0.0 {
                continue; // didn't fit its row
            }
            let lift = control_lift.get(i).copied().unwrap_or(0.0);
            let k = 1.0 + LIFT * lift;
            let (pw, ph) = (r.w * k, r.h * k);
            let rect = Rect::new(r.x + r.w / 2.0 - pw / 2.0, r.y + r.h / 2.0 - ph / 2.0, pw, ph);
            if panel_mix <= 0.0005 {
                let (blur, glow_a) = LAYER_GLOW[0];
                scene.overlay_shadows.push(ShadowInst {
                    rect,
                    radius: ph / 2.0,
                    blur: blur * sc,
                    color: [glow, glow, glow, glow_a * card_open * card_open * layout.controls_k],
                    edges: [1.0, 1.0, 1.0, 1.0],
                });
            }
            cgrid.rects.push(RectInst {
                rect,
                radius: ph / 2.0,
                color: crate::options::wash(!bright, lerp(rest_a, hover_a, lift) * layout.controls_k),
                glass: 0.0,
                border: 0.0,
            });
            cgrid.labels.push(Label {
                text: label.to_string(),
                pos: (rect.x + pw / 2.0, rect.y + (ph - line * k) / 2.0),
                max_w: pw,
                font_px: font * k,
                line_px: line * k,
                centered: true,
                dim: false,
                cache: (k - 1.0).abs() < 0.001,
                family: crate::options::TEXT_FONT,
                color: Some([ink[0], ink[1], ink[2], ink[3] * layout.controls_k]),
                clip: Some(reveal_clip(rect)),
            });
        }
        scene.grids.push(cgrid);
    }
    // The push: the settings field rises in from the card's bottom edge
    // and shoves the apps up and out under the dock; going back, the apps
    // come down from the top and push the field out through the bottom.
    // One travel for both, the card's own height, so they move as one.
    let m = panel_mix.clamp(0.0, 1.0);
    let travel = reveal_rect.h;
    if m > 0.0005 {
        let dy = -m * travel;
        // Clip each piece where it is now, but never past the card.
        let within = |c: Rect| -> Rect { reveal_clip(Rect::new(c.x, c.y + dy, c.w, c.h)) };
        for l in &mut scene.labels[apps_from.0..] {
            l.pos.1 += dy;
            l.clip = l.clip.map(within);
        }
        // Unclipped page dots would slide out over the dock: they go.
        scene.rects.truncate(apps_from.1);
        for g in &mut scene.grids[apps_from.2..] {
            g.clip = within(g.clip);
            for r in &mut g.rects {
                r.rect.y += dy;
            }
            for i in &mut g.icons {
                i.rect.y += dy;
            }
            for l in &mut g.labels {
                l.pos.1 += dy;
                l.clip = l.clip.map(within);
            }
        }
    }

    // The search pill, after the push so it stays where it is in both views.
    if let Some(g) = search_grid {
        scene.grids.push(g);
    }

    // The control panel's field (see `panel.rs`): its pills and open
    // setting clipped to the card like the sections they replace; their
    // glows arrive with the card, not before it (overlay shadows are not
    // clipped, so they also wait for the field to be in).
    if panel_draw.is_some() && m > 0.995 && card_open > 0.995 {
        picture_tiles(&mut scene, panel_images, panel_draw.map_or(0.0, |d| d.picture_radius), panel_draw.and_then(|d| d.card_chroma));
    }
    if let Some(pd) = panel_draw.filter(|_| m > 0.0005) {
        let rise = (1.0 - m) * travel;
        let arrive = card_open * card_open * m * m;
        for g in &pd.glows {
            let mut g = *g;
            g.color[3] *= arrive;
            g.rect.y += rise;
            scene.overlay_shadows.push(g);
        }
        let mut field = GridContent {
            clip: reveal_rect,
            ..Default::default()
        };
        field.rects.extend(pd.rects.iter().map(|r| {
            let mut r = *r;
            r.rect.y += rise;
            r
        }));
        field.labels.extend(pd.labels.iter().map(|l| {
            let mut l = l.clone();
            l.clip = Some(reveal_rect);
            l.pos.1 += rise;
            l
        }));
        // The apps' icons on their cards (Colours' Apps tiles): the dock's
        // own layers, bare — the card is their plate, as on fast launch.
        field.icons.extend(pd.icons.iter().map(|&(entry, r)| IconInst {
            rect: Rect::new(r.x, r.y + rise, r.w, r.h),
            layer: layer_of(entry),
            tint: [0.0; 4],
            ring: -1.0,
            plate: NO_PLATE,
        }));
        scene.grids.push(field);
        // Parts that scroll inside their own window (an open module's list).
        for (clip, rects, labels) in &pd.clipped {
            let mut c = *clip;
            c.y += rise;
            let (x0, y0) = (c.x.max(reveal_rect.x), c.y.max(reveal_rect.y));
            let x1 = (c.x + c.w).min(reveal_rect.x + reveal_rect.w);
            let y1 = (c.y + c.h).min(reveal_rect.y + reveal_rect.h);
            let c = Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
            let mut g = GridContent { clip: c, ..Default::default() };
            g.rects.extend(rects.iter().map(|r| {
                let mut r = *r;
                r.rect.y += rise;
                r
            }));
            g.labels.extend(labels.iter().map(|l| {
                let mut l = l.clone();
                l.clip = Some(c);
                l.pos.1 += rise;
                l
            }));
            scene.grids.push(g);
        }
    }

    // The open box: a rounded panel opening from the edge next to its tile,
    // its rows of member icons riding out from behind that edge. Drawn last
    // so it sits opaquely over the loose grid behind it.
    if let Some(box_rect) = open_box_rect {
        let t = group_expand.clamp(0.0, 1.0);
        // The corner of where it is open: the dock's over the dock, the
        // open card's in the grid.
        let radius = if box_over_dock {
            config.theme.corner_radius
        } else {
            lerp(14.0, BOX_CORNER_RADIUS, t)
        }
        .min(box_rect.h / 2.0);
        // Max, 2026-10-13: "centrados en la línea… desde el borde superior
        // se deslice hacia abajo", and over the dock "la misma animación
        // hacia arriba": the members ride out from behind the edge the box
        // opens from, whole from the start — nothing flies out of the tile.
        // In the grid that is its top edge, the members coming down; over
        // the dock its bottom edge, the members going up.
        let rest = layout.open_box.unwrap_or(box_rect);
        // (Beside a side dock the column's members ride out sideways, from
        // behind the edge by the dock.)
        let across = box_over_dock && layout.sideways();
        let (members_rect, members_clip) = if across {
            let slide = (1.0 - t) * rest.w * if layout.side == Side::Left { -1.0 } else { 1.0 };
            let r = Rect::new(rest.x + slide, rest.y, rest.w, rest.h);
            (r, Rect::new(box_rect.x, r.y, box_rect.w, r.h))
        } else {
            let slide = (1.0 - t) * rest.h * if box_over_dock { 1.0 } else { -1.0 };
            let r = Rect::new(rest.x, rest.y + slide, rest.w, rest.h);
            // What the members clip to: their own columns (pages slide past
            // the sides), no further than the box has opened.
            (r, Rect::new(r.x, box_rect.y, r.w, box_rect.h))
        };
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
        let mut panel = dock_bg;
        if !config.accessibility.reduce_transparency {
            panel[3] = crate::options::shell_glass(config.theme.open_alpha(panel[3]).max(dock_open_alpha));
        }
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
                color: [0.0, 0.0, 0.0, t * crate::options::shell_shade()],
                edges: [
                    DOCK_SHADOW_ALPHA_TOP,
                    DOCK_SHADOW_ALPHA_BOTTOM,
                    DOCK_SHADOW_ALPHA_SIDE,
                    DOCK_SHADOW_ALPHA_SIDE,
                ],
            });
        }
        let panel_inst = RectInst {
            rect: box_draw,
            radius,
            color: panel,
            glass: 0.5,
            border: 0.0,
        };
        // Its own grid: the members' clip is narrower than a line in the grid.
        // (A stack over the dock stands above the card's reveal region.)
        let panel_clip = if box_over_dock { Rect::new(0.0, 0.0, surface.0, surface.1) } else { reveal_rect };
        let mut line = GridContent {
            clip: panel_clip,
            ..Default::default()
        };
        line.rects.push(panel_inst);
        scene.grids.push(line);
        // Members read left to right, top to bottom, in their stored order.
        let (cols, rows) = (layout.open_box_grid.0.max(1), layout.open_box_grid.1.max(1));
        let cell = members_rect.w / cols as f32;
        let cell_h = members_rect.h / rows as f32;
        // Members lay out in a horizontal strip of pages shifted by
        // `box_scroll`, so paging slides. Everything clips to the box.
        let page_w = layout.open_box.map_or(box_rect.w, |b| b.w);
        boxgrid.clip = members_clip;
        // Members draw at their animated display slot (fractional
        // mid-glide — the reflow itself is computed in `draw`, shared
        // with the grid): positions lerp between the cell centers of the
        // two neighboring slots, pages included.
        // Each member stands as its neighbours do, its name under it: over
        // the dock a dock icon a size up, in a cell just wide enough for
        // the name; in the grid a grid icon in a grid cell, on the grid's
        // columns.
        let stack_scale = layout.open_box_scale;
        let member_icon = if box_over_dock { STACK_ICON * stack_scale } else { grid_icon };
        let icon_cy = if box_over_dock { STACK_ICON_TOP * stack_scale } else { grid_icon_top } + member_icon / 2.0;
        let name_w = cell - if box_over_dock { 6.0 } else { 12.0 };
        let name_scale = if box_over_dock { layout.open_box_text } else { 1.0 };
        let (name_px, name_line) = (LABEL_FONT_PX * name_scale, LABEL_LINE_PX * name_scale);
        let place = |di: usize| -> (f32, f32) {
            let cap = layout.open_box_cap.max(1);
            let slot = di % cap;
            let (row, col) = (slot / cols, slot % cols);
            let page_x = (di / cap) as f32 * page_w - box_scroll;
            (
                members_rect.x + page_x + (col as f32 + 0.5) * cell,
                members_rect.y + row as f32 * cell_h + icon_cy,
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
            if open_cx < members_rect.x - cell || open_cx > members_rect.x + members_rect.w + cell {
                continue;
            }
            let (cx, cy, size) = (open_cx, open_cy, member_icon);
            boxgrid.icons.push(IconInst {
                rect: Rect::new(cx - size / 2.0, cy - size / 2.0, size, size),
                layer: layer_of(idx),
                tint: [0.0; 4],
                ring: -1.0,
                plate: plate_of(idx),
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
                        family: None,
                        color: None,
                        clip: Some(members_clip),
                    });
                }
            }
            // Each member's name rides under its icon.
            if let Some(entry) = entries.get(idx) {
                boxgrid.labels.push(Label {
                    text: tile_label(entry, kinds.get(idx), name_w, name_px),
                    pos: (cx, cy + size / 2.0 + GRID_LABEL_GAP),
                    max_w: name_w,
                    font_px: name_px,
                    line_px: name_line,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: None,
                    color: None,
                    clip: Some(members_clip),
                });
            }
        }
        scene.grids.push(boxgrid);
        // Page dots (own grid so they aren't clipped to the box): between
        // a dock stack and its icon; on the bottom edge of a box in the
        // grid, where the row under it leaves them air.
        let (page, pages) = open_box_pages;
        if pages > 1 && t > 0.6 {
            let mut dots = GridContent {
                clip: panel_clip,
                ..Default::default()
            };
            let dot_r = 3.0;
            let spacing = 12.0;
            let total_w = pages as f32 * spacing - (spacing - dot_r * 2.0);
            let dy = box_rect.y + box_rect.h + if box_over_dock { BOX_GAP / 2.0 } else { 0.0 };
            let x0 = box_rect.x + box_rect.w / 2.0 - total_w / 2.0;
            let hl = dock_highlight;
            for p in 0..pages {
                let x = x0 + p as f32 * spacing + dot_r;
                let a = if p == page { hl[3] } else { hl[3] * 0.35 };
                dots.rects.push(RectInst {
                    rect: Rect::new(x - dot_r, dy - dot_r, dot_r * 2.0, dot_r * 2.0),
                    radius: dot_r,
                    color: [hl[0], hl[1], hl[2], a],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            scene.grids.push(dots);
        }
    }

    // FAST LAUNCH: the glass card mid-screen, the bare icons on it, and the
    // word at its bottom, over everything.
    if let Some(f) = fast.filter(|f| f.a > 0.0) {
        let a = f.a;
        let dock_ink = f.ink;
        // CLEAR GLASS AT ITS DENSEST (Max, 2026-10-21: "make less transparent
        // the plate on fast launcher", "less transparent", then of the open
        // box's frost "that is too much"): just under the compositor's blur
        // threshold, so no blur — the full glass material, and the rim the
        // plates and the dock wear. (The resting dock's 0.65 since
        // 2026-10-02: "more transparent" → "i want that".)
        let mut fill = dock_bg;
        if !config.accessibility.reduce_transparency {
            fill[3] = crate::options::shell_glass(fill[3] * FAST_FILL);
        }
        fill[3] *= a;
        scene.rects.push(RectInst { rect: f.card, radius: f.radius, color: fill, glass: 1.0, border: 0.0 });
        scene.rects.push(RectInst { rect: f.card, radius: f.radius, color: plate_rim(fill), glass: 0.0, border: 1.0 });
        for &(entry, rect) in f.icons {
            scene.overlay.push(IconInst {
                rect,
                layer: layer_of(entry),
                tint: [0.0; 4],
                ring: -1.0,
                plate: NO_PLATE, // the card is its plate
            });
        }
        // QUICK CONTROLS: the settings as WORDS (Max, 2026-10-21: "change the
        // fast launch pills for those underlined sections"), as the Colours
        // box's tabs are: the front one bright and underlined — what was
        // typed bright, the rest of its name dimmed — the ones beside it
        // dimmed. Through the clear card they wear the launcher word's halo.
        let light_ink = dock_ink[0] + dock_ink[1] + dock_ink[2] > 1.5;
        let halo_rgb = if light_ink { [0.0, 0.0, 0.0] } else { [1.0, 1.0, 1.0] };
        for p in f.pills {
            let r = p.rect;
            let show = p.show * a;
            let line = p.font_px * 1.2;
            let top = r.y + (r.h - line) / 2.0;
            let left = r.x + (r.w - (p.typed_w + p.rest_w)) / 2.0;
            let halo_r = (p.font_px * 0.07).max(1.0);
            let ink = |k: f32| [dock_ink[0], dock_ink[1], dock_ink[2], dock_ink[3] * k * show];
            let halo = |k: f32| [halo_rgb[0], halo_rgb[1], halo_rgb[2], 0.28 * k * show];
            let mut part = |text: &str, x: f32, k: f32| {
                if text.is_empty() {
                    return;
                }
                let d = halo_r * std::f32::consts::FRAC_1_SQRT_2;
                let rings = [(halo_r, 0.0), (-halo_r, 0.0), (0.0, halo_r), (0.0, -halo_r), (d, d), (-d, d), (d, -d), (-d, -d)];
                let copies = rings.iter().map(|&o| (o, halo(k))).chain(std::iter::once(((0.0, 0.0), ink(k))));
                for ((dx, dy), color) in copies {
                    scene.labels.push(Label {
                        text: text.to_owned(),
                        pos: (x + dx, top + dy),
                        max_w: r.w + p.font_px,
                        font_px: p.font_px,
                        line_px: line,
                        centered: false,
                        dim: false,
                        cache: false,
                        family: crate::options::TEXT_FONT,
                        color: Some(color),
                        clip: None,
                    });
                }
            };
            if p.front {
                // Nothing typed: the whole name bright, as on the panel.
                let rest_k = if p.typed.is_empty() { 1.0 } else { 0.45 };
                part(&p.typed, left, 1.0);
                part(&p.rest, left + p.typed_w, rest_k);
                // The underline: the tabs' hairline (1 px at their 13 px),
                // under the whole name.
                let (uw, th) = (p.typed_w + p.rest_w, (p.font_px / 13.0).max(1.0));
                let uy = top + line + p.font_px * 0.12;
                scene.rects.push(RectInst { rect: Rect::new(left - halo_r, uy - halo_r, uw + 2.0 * halo_r, th + 2.0 * halo_r), radius: 0.0, color: halo(1.0), glass: 0.0, border: 0.0 });
                scene.rects.push(RectInst { rect: Rect::new(left, uy, uw, th), radius: 0.0, color: ink(1.0), glass: 0.0, border: 0.0 });
            } else {
                part(&p.rest, left, 0.55);
            }
        }
        if !f.pills.is_empty() {
            return scene;
        }
        // The whole word centred (it never jumps as you type: "sp" already
        // sits where "Spotify" will be), and always whole (Max, 2026-10-02:
        // "the titles dont show complete"): the card widens round a long
        // name (`FastLaunch::wide`).
        let b = f.text_box;
        let total = f.typed_w + f.rest_w;
        let left = b.x + (b.w - total) / 2.0;
        // Readable over anything through the clear glass (Max, 2026-10-02,
        // "1 and 2"): a HALO — the word drawn eight times round itself in
        // the opposite of the ink, a hair out, like subtitles over a film —
        // and the dimmed rest of the name lifted to 65% (then "a little
        // lighter" halo, 40% → 28%, and the completion dimmer, 50% → 40%).
        let halo_r = (f.font_px * 0.07).max(1.0);
        let light_ink = dock_ink[0] + dock_ink[1] + dock_ink[2] > 1.5;
        let halo_rgb = if light_ink { [0.0, 0.0, 0.0] } else { [1.0, 1.0, 1.0] };
        let mut word = |text: &str, x: f32, color: [f32; 4]| {
            if text.is_empty() {
                return;
            }
            let halo = [halo_rgb[0], halo_rgb[1], halo_rgb[2], 0.28 * color[3]];
            let d = halo_r * std::f32::consts::FRAC_1_SQRT_2;
            let rings = [
                (halo_r, 0.0), (-halo_r, 0.0), (0.0, halo_r), (0.0, -halo_r),
                (d, d), (-d, d), (d, -d), (-d, -d),
            ];
            let copies = rings.iter().map(|&o| (o, halo)).chain(std::iter::once(((0.0, 0.0), color)));
            for ((dx, dy), c) in copies {
                scene.labels.push(Label {
                    text: text.to_owned(),
                    pos: (x + dx, b.y + dy),
                    max_w: total + 4.0 * halo_r + f.font_px,
                    font_px: f.font_px,
                    line_px: f.line_px,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: crate::options::TEXT_FONT,
                    color: Some(c),
                    clip: None,
                });
            }
        };
        let ink = |k: f32| [dock_ink[0], dock_ink[1], dock_ink[2], dock_ink[3] * k * a];
        word(f.typed, left, ink(0.95));
        word(f.rest, left + f.typed_w, ink(0.4));
    }

    // THE QUICK BOX: one setting mid-screen, in the LAUNCHER's own material
    // (Max, 2026-10-13: "más transparentes, como el lanzador, pero no los
    // contenidos") — the open card's fill and glass, the rim the plates
    // wear. What is ON it stays solid: every choice and preview frame gets
    // a plate of the dock's colour under its wash, so it reads the same
    // over any window.
    if let Some(q) = quick.filter(|q| q.a > 0.0) {
        let dock_bg = q.bg;
        let mut fill = dock_bg;
        if !config.accessibility.reduce_transparency {
            fill[3] = crate::options::shell_glass(config.theme.open_alpha(fill[3]));
        }
        fill[3] *= q.a;
        // Everything about the box's centre, at the size it is this frame.
        let k = q.scale;
        let (cx, cy) = (q.rect.x + q.rect.w / 2.0, q.rect.y + q.rect.h / 2.0);
        let at = |x: f32, y: f32| (cx + (x - cx) * k, cy + (y - cy) * k);
        let place = |r: Rect| {
            let (x, y) = at(r.x, r.y);
            Rect::new(x, y, r.w * k, r.h * k)
        };
        let still = (k - 1.0).abs() < 0.001;
        let radius = BOX_CORNER_RADIUS * k;
        let frame = place(q.rect);
        scene.rects.push(RectInst { rect: frame, radius, color: fill, glass: 1.0, border: 0.0 });
        scene.rects.push(RectInst { rect: frame, radius, color: plate_rim(fill), glass: 0.0, border: 1.0 });
        for r in &q.draw.rects {
            let mut r = *r;
            r.rect = place(r.rect);
            r.radius *= k;
            if r.border == 0.0 {
                scene.rects.push(RectInst {
                    rect: r.rect,
                    radius: r.radius,
                    color: [dock_bg[0], dock_bg[1], dock_bg[2], QUICK_SOLID * q.a],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            r.color[3] *= q.a;
            scene.rects.push(r);
        }
        let write = |scene: &mut Scene, mut l: Label| {
            l.pos = at(l.pos.0, l.pos.1);
            l.max_w *= k;
            l.font_px *= k;
            l.line_px *= k;
            l.clip = l.clip.map(place);
            // (A size that changes every frame is not worth keeping.)
            l.cache &= still;
            if let Some(c) = &mut l.color {
                c[3] *= q.a;
            }
            // (Too small to read is too small to shape.)
            if l.font_px >= 2.0 {
                scene.labels.push(l);
            }
        };
        for l in &q.draw.labels {
            write(&mut scene, l.clone());
        }
        // Its pictures are on it while it is more there than not (a
        // picture has no opacity of its own — and a tint is the mark of an
        // app about to be removed, never a way to dim one).
        if q.a > 0.5 {
            picture_tiles_as(&mut scene, q.images, q.draw.picture_radius * k, q.draw.card_chroma, place);
            for &(entry, r) in &q.draw.icons {
                scene.overlay.push(IconInst { rect: place(r), layer: layer_of(entry), tint: [0.0; 4], ring: -1.0, plate: NO_PLATE });
            }
            for (tile, name) in q.names {
                let px = (tile.h * 0.11).clamp(9.0, 14.0);
                write(
                    &mut scene,
                    Label {
                        text: name.clone(),
                        pos: (tile.x + tile.w / 2.0, tile.y + (tile.h - px * 1.25) / 2.0),
                        max_w: tile.w - px,
                        font_px: px,
                        line_px: px * 1.25,
                        centered: true,
                        dim: false,
                        cache: false,
                        family: crate::options::TEXT_FONT,
                        color: Some([q.ink[0], q.ink[1], q.ink[2], q.ink[3] * 0.5]),
                        clip: Some(*tile),
                    },
                );
            }
        }
    }

    // Ghost of a box member being reordered, following the pointer (topmost).
    if let Some((entry, _, pos)) = box_drag {
        let s = grid_icon * 1.1;
        scene.overlay.push(IconInst {
            rect: Rect::new(pos.0 - s / 2.0, pos.1 - s / 2.0, s, s),
            layer: layer_of(entry),
            tint: [0.0; 4],
            ring: -1.0,
            plate: plate_of(entry),
        });
    }

    // THE DOCK'S ICONS at the Icons section's "Dock icons" (Max, 2026-10-27:
    // "mis iconos en el dock un poco más transparentes" — the Shell as it is).
    let dock = crate::options::dock_icons();
    if dock < 1.0 {
        scene.icon_fade = vec![dock; scene.icons.len()];
    }
    // The glows on its pills fade with them (the shader leaves a shadow at
    // the strength it was given).
    crate::options::shell_halos(&mut scene);

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

    /// The open card with `n` apps in the grid, under a dock that fits the
    /// box (a wider dock widens the card and its grid — tested on its own).
    fn open_layout(cfg: &Config, n: usize, scroll: f32) -> Layout {
        layout(
            cfg,
            1.0,
            SURFACE,
            OPEN,
            n.min(10),
            0, 0, 0.0,
            &[],
            [n, 0, 0],
            [scroll, 0.0, 0.0],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
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
            0, 0, 0.0,
            &[],
            [20, 0, 6],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
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
    fn a_search_with_no_app_gives_files_a_second_line() {
        let cfg = config();
        let at = |files: usize, trim: f32| {
            layout(&cfg, 1.0, SURFACE, OPEN - trim, 10, 0, 0, 0.0, &[], [0, 1, files], [0.0; N_SECTIONS], false, (1.0, 1.0), None, 0.0, trim, 1)
        };
        let cell = GRID_CELL_H;
        let bottom = |v: Rect| v.y + v.h;
        // The trim the App aims at: the "No results" line and a row for Files.
        let full = at(12, 0.0);
        let trim = full.sections[SECTION_APPS].viewport.h - NO_APPS_H - cell;
        let before = at(12, trim);
        let mut l = at(12, trim);
        files_take_apps_row(&mut l, cell, 0.0);
        let (apps, files) = (&l.sections[SECTION_APPS], &l.sections[SECTION_FILES]);
        assert!((apps.viewport.h - NO_APPS_H).abs() < 0.01, "Apps is its short line");
        assert_eq!((files.rows, files.cap, files.n_pages), (2, 2 * files.cols, 1));
        assert!((bottom(files.viewport) - bottom(before.sections[SECTION_FILES].viewport)).abs() < 0.01, "Files' bottom stays put");
        assert!(l.sections[SECTION_INSTALL].viewport.y >= bottom(apps.viewport), "Install under Apps");
        assert!(files.viewport.y >= bottom(l.sections[SECTION_INSTALL].viewport), "Files under Install");
        assert_eq!(l.sections[SECTION_INSTALL].rows, 1, "Install keeps one line");
        // Half open: Install and Files half way up, Files' bottom where it was.
        let mut half = at(12, trim);
        files_take_apps_row(&mut half, cell / 2.0, 0.0);
        let dy = before.sections[SECTION_INSTALL].viewport.y - half.sections[SECTION_INSTALL].viewport.y;
        assert!((dy - cell / 2.0).abs() < 0.01);
        assert!((bottom(half.sections[SECTION_FILES].viewport) - bottom(before.sections[SECTION_FILES].viewport)).abs() < 0.01);
        // Never more than the Apps' room.
        let mut short = at(12, full.sections[SECTION_APPS].viewport.h - 20.0);
        files_take_apps_row(&mut short, cell, 0.0);
        assert!(short.sections[SECTION_APPS].viewport.h.abs() < 0.01);
        // The pager's own position, wrapped for the two-line pages: a wheel
        // that wrapped forward to -w is on the second page of two.
        let cols = full.sections[SECTION_FILES].cols;
        let mut wrapped = at(2 * cols + 3, trim);
        let w = wrapped.sections[SECTION_FILES].viewport.w;
        files_take_apps_row(&mut wrapped, cell, -w);
        assert_eq!(wrapped.sections[SECTION_FILES].n_pages, 2);
        assert!((wrapped.sections[SECTION_FILES].scroll - w).abs() < 0.01);
    }

    #[test]
    fn a_controls_row_takes_shown_rows_not_page_capacity() {
        let cfg = config();
        let args = |c: Option<f32>| {
            layout(&cfg, 1.0, SURFACE, OPEN, 10, 0, 0, 0.0, &[], [40, 1, 6], [0.0; N_SECTIONS], false, (1.0, 1.0), c, c.map_or(0.0, |ph| controls_band_h(1.0, ph)), 0.0, 1)
        };
        let (plain, with) = (args(None), args(Some(23.0)));
        assert!(plain.control_bands.is_empty());
        let row = with.control_bands.first().map(|b| b.1).expect("a row while controls match");
        let files = &with.sections[SECTION_FILES].viewport;
        assert!(row.y >= files.y + files.h, "the row sits under Files");
        assert!(row.y + row.h <= with.search_box.y, "and above the search pill");
        assert_eq!(
            with.sections[SECTION_APPS].cap, plain.sections[SECTION_APPS].cap,
            "page capacity never moves with the row"
        );
    }

    #[test]
    fn a_trimmed_card_drops_its_top_and_keeps_its_bottom() {
        let cfg = config();
        let at = |trim: f32| {
            layout(&cfg, 1.0, SURFACE, OPEN - trim, 10, 0, 0, 0.0, &[], [40, 1, 6], [0.0; N_SECTIONS], false, (1.0, 1.0), None, 0.0, trim, 1)
        };
        let (full, short) = (at(0.0), at(GRID_CELL_H));
        let (a, b) = (&full.sections[SECTION_APPS], &short.sections[SECTION_APPS]);
        assert!((b.viewport.y - a.viewport.y - GRID_CELL_H).abs() < 0.5, "the Apps grid comes down a row");
        assert!((a.viewport.h - b.viewport.h - GRID_CELL_H).abs() < 0.5, "and is a row shorter");
        assert_eq!(a.cap, b.cap, "page capacity never moves");
        let (i, j) = (&full.sections[SECTION_INSTALL], &short.sections[SECTION_INSTALL]);
        assert!((i.viewport.y - j.viewport.y).abs() < 0.5, "Install stays put");
        assert!((full.search_box.y - short.search_box.y).abs() < 0.5, "so does Search");
    }

    #[test]
    fn search_rows_split_about_the_title() {
        let cfg = config();
        let mut l = open_layout(&cfg, 3, 0.0);
        let sec = &mut l.sections[SECTION_APPS];
        let (w, cell) = (sec.viewport.w, sec.viewport.w / sec.cols as f32);
        assert_eq!(sec.row_shift(0), 0.0, "the resting grid never moves");
        sec.centre_rows = true;
        for n in 2..6usize {
            sec.cells = n;
            let mid = sec.row_shift(0) + n as f32 * cell / 2.0;
            assert!((mid - w / 2.0).abs() < 0.01, "{n}: the row's middle is under the title");
        }
        sec.cells = 1;
        assert!((sec.row_shift(0) + cell / 2.0 - w / 2.0).abs() < 0.01, "one result under the title");
        sec.cells = sec.cols + 1;
        assert_eq!(sec.row_shift(0), 0.0, "a full row stays");
        assert!(sec.row_shift(sec.cols) > 0.0, "the partial row after it splits");
    }

    #[test]
    fn overflowing_pins_fold_into_the_apps_tile() {
        let cfg = config();
        // 40 entries: 30 pins, the trio, 7 running apps — far wider than the
        // 880 px surface.
        let at = |off: f32| {
            layout(&cfg, 1.0, SURFACE, 48.0, 40, 0, 30, off, &[], [0, 0, 0], [0.0; N_SECTIONS], false, (1.0, 1.0), None, 0.0, 0.0, 1)
        };
        let rest = at(0.0);
        let ov = rest.dock_overflow.expect("30 pins overflow 880 px");
        assert_eq!(ov.apps_slot, 30, "the Apps tile closes the pins");
        assert!(ov.hidden_right > 0 && ov.max_off > 0.0);
        let row: f32 = rest.dock_slots.iter().map(|r| r.w).sum();
        assert!(row <= SURFACE.0 - 4.0 * DOCK_PAD_X + 0.5, "the row fits: {row}");
        assert!((rest.dock_slots[30].w - DOCK_SLOT).abs() < 0.01, "the trio keeps whole slots");
        assert!((7..40).skip(24).all(|i| rest.dock_slot_f[i] >= 0.999), "the running zone never folds");
        // Slid to the far end: everything folded is out; the first pins left.
        let end = at(ov.max_off);
        assert_eq!(end.dock_overflow.unwrap().hidden_right, 0);
        assert!(end.dock_slot_f[0] < 0.01 && end.dock_slot_f[29] > 0.99);
        // Half a slot along, the two ends share one slot between them.
        let half = at(0.5);
        let w: f32 = half.dock_slots.iter().map(|r| r.w).sum();
        assert!((w - row).abs() < 0.5, "the window keeps its width while sliding");
    }

    #[test]
    fn the_controls_split_about_the_title() {
        let row = Rect::new(0.0, 0.0, 600.0, 30.0);
        let gap = 10.0;
        let centre = |r: &[Rect], after: usize| (r[after - 1].x + r[after - 1].w + r[after].x) / 2.0;
        let one = controls_rects(row, &[80.0], gap);
        assert_eq!(one[0].x + one[0].w / 2.0, 300.0, "one pill centres on the title");
        for n in [2, 4, 6] {
            let r = controls_rects(row, &vec![60.0; n], gap);
            assert_eq!(r.len(), n);
            assert!((centre(&r, n / 2) - 300.0).abs() < 0.01, "{n}: the middle gap sits on the title");
        }
        // Odd: the middle pill on the title, whatever its neighbours' widths.
        let r = controls_rects(row, &[120.0, 50.0, 90.0], gap);
        assert!((r[1].x + r[1].w / 2.0 - 300.0).abs() < 0.01, "3: the middle pill on the title");
        // Too wide for both halves: the best that fit are kept.
        assert_eq!(controls_rects(row, &[200.0; 4], gap).len(), 2);
    }

    #[test]
    fn the_system_row_passes_over_a_pill_too_wide_and_takes_the_next() {
        let row = Rect::new(0.0, 0.0, 600.0, 30.0);
        // The second is too wide beside the first; the third still fits.
        assert_eq!(row_fits(row, &[150.0, 500.0, 120.0], 10.0), vec![0, 2]);
        // Everything that fits, in order.
        assert_eq!(row_fits(row, &[100.0, 100.0, 100.0], 10.0), vec![0, 1, 2]);
        assert!(row_fits(row, &[700.0], 10.0).is_empty(), "wider than the row: none");
    }

    #[test]
    fn open_extent_shows_three_sections() {
        let cfg = config();
        let l = open_layout(&cfg, 40, 1e9);
        let apps = &l.sections[SECTION_APPS];
        assert_eq!(apps.cols, 6, "720px card fits exactly 6 columns");
        assert_eq!(apps.rows, 3, "the mockup spacing leaves the default card 6×3");
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
            0, 0, 0.0,
            &[],
            [4, 0, 6],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
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
            0, 0, 0.0,
            &[],
            [10, 0, 6],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
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
    fn section_at_routes_scroll_bands() {
        let cfg = config();
        let l = layout(
            &cfg,
            1.0,
            SURFACE,
            OPEN,
            10,
            0, 0, 0.0,
            &[],
            [10, 0, 6],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
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

    #[test]
    fn scroll_to_reveal_lands_on_the_cell_s_page_and_clamps() {
        // Enough entries to guarantee several pages (as scrolled_hit_test
        // does with 18*3): read cells-per-page from the built section.
        let cfg = config();
        let sec = &open_layout(&cfg, 18 * 3, 0.0).sections[SECTION_APPS];
        let per = sec.cols * sec.rows;
        assert!(sec.n_pages >= 2, "test needs a multi-page section");
        // Cell on page 0 → no scroll; a cell on page 1 → one viewport width.
        assert_eq!(scroll_to_reveal(sec, 0), 0.0);
        assert!((scroll_to_reveal(sec, per) - sec.viewport.w).abs() < 0.01);
        // A cell index past the last page clamps to the last page, never
        // scrolls into empty space beyond the content.
        let last_page_scroll = (sec.n_pages - 1) as f32 * sec.viewport.w;
        assert!((scroll_to_reveal(sec, per * 999) - last_page_scroll).abs() < 0.01);
    }

    #[test]
    fn open_box_slots_read_row_by_row() {
        let b = Rect::new(100.0, 50.0, 500.0, 200.0); // 5 × 2 cells of 100
        assert_eq!(open_box_slot(b, (5, 2), (110.0, 60.0)), 0);
        assert_eq!(open_box_slot(b, (5, 2), (590.0, 60.0)), 4);
        assert_eq!(open_box_slot(b, (5, 2), (110.0, 240.0)), 5);
        assert_eq!(open_box_slot(b, (5, 2), (590.0, 240.0)), 9);
    }

    #[test]
    fn row_box_is_one_row_over_its_tile() {
        let area = Rect::new(80.0, 0.0, 840.0, 500.0);
        let icon = Rect::new(480.0, 500.0, 40.0, 40.0); // centre x = 500
        let (r, grid, _) = row_box(icon, 5, (80.0, 80.0), area, 12.0);
        assert!((r.w - 400.0).abs() < 0.01);
        assert_eq!(grid, (5, 1));
        assert!((r.x + r.w / 2.0 - 500.0).abs() < 0.01);
        assert!((r.y + r.h - (icon.y - 12.0)).abs() < 0.01, "sits just over the tile");
        // A tile near the edge keeps the row inside the area.
        let edge = Rect::new(900.0, 500.0, 40.0, 40.0);
        let (r, _, _) = row_box(edge, 5, (80.0, 80.0), area, 12.0);
        assert!(r.x + r.w <= area.x + area.w + 0.01);
    }

    #[test]
    fn row_cells_fill_the_line_before_a_page_is_needed() {
        // A few members: as many cells.
        let ((w, h), grid, cap) = row_cells(4, (80.0, 90.0), 560.0);
        assert_eq!((grid, cap), ((4, 1), 7));
        assert!((w - 320.0).abs() < 0.01 && (h - 90.0).abs() < 0.01);
        // Thirty: the line is as full as it gets, the rest on next pages.
        let ((w, h), grid, cap) = row_cells(30, (80.0, 90.0), 560.0);
        assert_eq!((grid, cap), ((7, 1), 7));
        assert!((w - 560.0).abs() < 0.01 && (h - 90.0).abs() < 0.01);
    }

    #[test]
    fn parted_rows_map_back_to_where_they_rest() {
        // Rows of 80 from y = 100, parted under row 1: 10 up, 90 down.
        let part = AppsPart { row: 1, up: 10.0, down: 90.0 };
        assert_eq!(part.shift(0), -10.0);
        assert_eq!(part.shift(1), -10.0);
        assert_eq!(part.shift(2), 90.0);
        // Row 1 is drawn at 170..250; the room made is 250..350.
        assert_eq!(part.rest_y(100.0, 80.0, 200.0), Some(210.0));
        assert_eq!(part.rest_y(100.0, 80.0, 300.0), None);
        assert_eq!(part.rest_y(100.0, 80.0, 360.0), Some(270.0));
        // A box from the dock: every row is below it.
        let part = AppsPart { row: -1, up: 0.0, down: 90.0 };
        assert_eq!(part.shift(0), 90.0);
        assert_eq!(part.rest_y(100.0, 80.0, 150.0), None);
    }

    /// The card and the row it holds are one movement (Max, 2026-09-13).
    /// The *acceleration* is shared by construction — a single eased
    /// `dock_mag` feeds both, so there is no second animator that could
    /// lag — which leaves the distance as the thing a test can pin: each
    /// card edge travels `DOCK_SPREAD_FOLLOW` of what the outer icon
    /// beside it travels, symmetrically, wherever the crest happens to be.
    #[test]
    fn magnification_carries_the_card_edges_with_the_row() {
        let cfg = config();
        let n = 9;
        let l = layout(
            &cfg,
            1.0,
            SURFACE,
            48.0,
            n,
            0, 0, 0.0,
            &[],
            [n, 0, 0],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
        );
        assert_eq!(l.dock_slots.len(), n, "every entry gets a slot");
        let es = entries(n);
        let order = idslots(n);
        // The card fill plus the row's two outer edges, for a given set of
        // per-slot magnification scales.
        let shot = |mag: &[f32]| {
            let s = scene(
                &cfg,
                1.0,
                &l,
                &es,
                &vis(n),
                SURFACE,
                &FrameInput {
                    alpha: 1.0,
                    dock_order: &order,
                    dock_mag: mag,
                    ..Default::default()
                },
            );
            let first = s.icons.first().expect("dock icons drawn").rect;
            let last = s.icons.last().expect("dock icons drawn").rect;
            (s.rects[0].rect, first.x, last.x + last.w)
        };

        let (rest_card, rest_l, rest_r) = shot(&vec![1.0; n]);
        assert!(
            (rest_card.x - l.card_x).abs() < 0.01 && (rest_card.w - l.card_w).abs() < 0.01,
            "an unmagnified row leaves the card exactly at its layout rect"
        );

        // A crest under the middle icon, melting away to both sides — the
        // same cosine falloff the pointer drives.
        let mag: Vec<f32> = (0..n)
            .map(|i| {
                1.0 + (DOCK_MAGNIFY - 1.0) * falloff((i as f32 - 4.0) * DOCK_SLOT, DOCK_MAG_RADIUS)
            })
            .collect();
        let (card, l_edge, r_edge) = shot(&mag);

        let row_left_travel = rest_l - l_edge;
        let row_right_travel = r_edge - rest_r;
        let card_left_travel = rest_card.x - card.x;
        let card_right_travel = (card.x + card.w) - (rest_card.x + rest_card.w);
        assert!(
            row_left_travel > 1.0,
            "the fixture must actually spread the row ({row_left_travel}px)"
        );
        // The card takes its fixed share of whatever the row travelled —
        // stated against the constant, so retuning the follow retunes the
        // test with it and only a *decoupling* can fail here.
        assert!(
            (row_left_travel * DOCK_SPREAD_FOLLOW - card_left_travel).abs() < 0.01,
            "left edge: row moved {row_left_travel}px, card {card_left_travel}px \
             (want {DOCK_SPREAD_FOLLOW}× the row)"
        );
        assert!(
            (row_right_travel * DOCK_SPREAD_FOLLOW - card_right_travel).abs() < 0.01,
            "right edge: row moved {row_right_travel}px, card {card_right_travel}px \
             (want {DOCK_SPREAD_FOLLOW}× the row)"
        );
        assert!(
            (card_left_travel - card_right_travel).abs() < 0.01,
            "the card must grow symmetrically wherever the crest sits"
        );
        // …and the grown card still fits the fixed-size surface.
        assert!(
            card.x >= 0.0 && card.x + card.w <= SURFACE.0 + 0.01,
            "card grew off the surface: {card:?}"
        );
    }

    #[test]
    fn minimized_tiles_widen_the_dock_and_are_never_clamped() {
        let cfg = config();
        let docked = 48.0;
        // Square tiles (aspect 1.0) so each minimized slot equals the uniform
        // `DOCK_SLOT` — this test pins the widen/clamp behaviour, which is
        // independent of the tile shapes (covered by the aspect test below).
        let lay = |total: usize, n_min: usize| {
            let aspects = vec![1.0f32; n_min];
            layout(
                &cfg,
                1.0,
                SURFACE,
                docked,
                total,
                n_min, 0, 0.0,
                &aspects,
                [total, 0, 0],
                [0.0; N_SECTIONS],
                false,
                (1.0, 1.0),
                None, 0.0,
                0.0,
                1,
            )
        };

        // 17 tiles, 7 of them minimized. The base card width (w − drag
        // margins) fits only 15; the old single clamp dropped the 2 tail
        // (minimized) tiles even though the surface has room. Now every
        // minimized tile keeps its slot.
        let with_min = lay(17, 7);
        assert_eq!(
            with_min.dock_slots.len(),
            17,
            "minimized tiles are never clamped away while the surface has room"
        );
        // The SAME count as plain pins widens the dock the same way: pinned
        // icons are no longer clamped to the card width either — the tail of
        // the dock (the Recycle Bin) used to vanish as it filled (2026-09-29).
        let all_normal = lay(17, 0);
        assert_eq!(
            all_normal.dock_slots.len(),
            17,
            "pinned/running icons widen the dock instead of being dropped"
        );
        // Risen, the card carries that row on top: it may never gather
        // narrower than the row, or the outer icons hang off its sides.
        let open = layout(
            &cfg,
            1.0,
            SURFACE,
            (cfg.window.height + cfg.window.bottom_margin) as f32,
            17,
            0, 0, 0.0,
            &[],
            [17, 0, 0],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
        );
        let row = open.dock_slots.last().unwrap().x + open.dock_slots.last().unwrap().w
            - open.dock_slots[0].x;
        assert!(
            open.card_w >= row,
            "open card ({}) narrower than its dock row ({row})",
            open.card_w
        );
        // …and the grid inside spreads to the sides with it: more columns
        // than a card whose dock fits the box.
        let narrow = layout(
            &cfg,
            1.0,
            SURFACE,
            (cfg.window.height + cfg.window.bottom_margin) as f32,
            3,
            0, 0, 0.0,
            &[],
            [17, 0, 0],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
        );
        assert!(
            open.sections[SECTION_APPS].cols > narrow.sections[SECTION_APPS].cols,
            "a wide dock widens the grid: {} cols vs {}",
            open.sections[SECTION_APPS].cols,
            narrow.sections[SECTION_APPS].cols
        );

        // Enough minimized WIDTH to push the row past the default resting
        // basin: the card stretches to wrap it (docked, so card_w == basin).
        // Wide-aspect tiles (real windows are rarely square), so a handful
        // suffice regardless of the tile-height scale.
        let lay_wide = |total: usize, n_min: usize| {
            let aspects = vec![MAX_TILE_ASPECT; n_min];
            layout(
                &cfg, 1.0, SURFACE, docked, total, n_min, 0, 0.0, &aspects, [total, 0, 0], [0.0; N_SECTIONS], false, (1.0, 1.0),
                None, 0.0,
                0.0,
                1,
            )
        };
        let stretched = lay_wide(19, 9); // wide tiles are width-capped now, so more are needed to overflow
        let small = lay(8, 0);
        assert!(
            stretched.card_w > small.card_w,
            "the dock stretches ({} → {}) to wrap the wide minimized row",
            small.card_w,
            stretched.card_w
        );
        // …but never off the fixed-size surface (a dock-padding margin aside).
        assert!(
            stretched.card_x >= 0.0 && stretched.card_x + stretched.card_w <= SURFACE.0 + 0.01,
            "the stretched card stays on the surface: {stretched:?}"
        );
    }

    /// Each minimized tile carries its window's shape: a wide window tiles
    /// wider than a tall one, while the pinned zone stays uniform — and the
    /// same widths reach the drawn thumbnails, with the app-icon badge only
    /// where the class resolved.
    #[test]
    fn minimized_tile_width_follows_the_window_aspect() {
        let cfg = config();
        // 3 pins + 2 minimized tiles (a wide window, a tall one).
        let aspects = [2.0f32, 0.5];
        let l = layout(
            &cfg,
            1.0,
            SURFACE,
            48.0,
            5,
            2, 0, 0.0,
            &aspects,
            [5, 0, 0],
            [0.0; N_SECTIONS],
            false,
            (1.0, 1.0),
            None, 0.0,
            0.0,
            1,
        );
        let n = l.dock_slots.len();
        assert_eq!(n, 5, "all five tiles are shown");
        let (wide_slot, tall_slot, pin) = (l.dock_slots[3], l.dock_slots[4], l.dock_slots[0]);
        assert!(
            wide_slot.w > tall_slot.w,
            "a wide window tiles wider than a tall one ({} vs {})",
            wide_slot.w,
            tall_slot.w
        );
        assert!(
            (pin.w - DOCK_SLOT).abs() < 0.01,
            "the pinned zone keeps the uniform square slot"
        );
        assert!(
            wide_slot.w > pin.w && tall_slot.w < pin.w,
            "the wide tile exceeds and the tall tile trails the square slot"
        );

        // scene() draws the thumbnails at those same widths, and badges only
        // the tile whose class resolved (here: the wide one).
        let es = entries(5);
        let order = idslots(5);
        let s = scene(
            &cfg,
            1.0,
            &l,
            &es,
            &vis(5),
            SURFACE,
            &FrameInput {
                alpha: 1.0,
                stretch: 1.0, // rest: the default 0.0 collapses tile heights
                dock_order: &order,
                min_badges: &[Some(7), None],
                min_aspects: &aspects,
                ..Default::default()
            },
        );
        // Draw order: 3 pins, then (wide thumb, wide badge), then tall thumb.
        assert_eq!(s.icons.len(), 6, "3 pins + 2 thumbs + 1 badge");
        let (wide_thumb, badge, tall_thumb) = (s.icons[3].rect, s.icons[4].rect, s.icons[5].rect);
        assert!(
            wide_thumb.w > tall_thumb.w,
            "the drawn wide thumbnail is wider than the tall one ({} vs {})",
            wide_thumb.w,
            tall_thumb.w
        );
        assert!(
            badge.w < wide_thumb.h && badge.w < wide_thumb.w,
            "the badge is a small corner overlay, not the whole tile"
        );
        assert_eq!(s.icons[3].layer, 3, "the thumbnail keeps the tile's own layer");
        assert_eq!(s.icons[4].layer, 7, "the badge draws the resolved app layer");
    }

    // ---- the dock on a side (Max, 2026-10-21) ----

    const SCREEN: (f32, f32) = (1440.0, 900.0);
    /// The banner over a side dock's column.
    const BANNER: f32 = 28.0;

    fn side_layout(side: Side, extent: f32, apps: usize, scroll: f32) -> Layout {
        let cfg = config();
        layout_on(side, &cfg, 1.0, SCREEN, extent, 9, 0, 0, 0.0, &[], [apps, 6, 9], [scroll, 0.0, 0.0], false, (1.0, 1.0), None, 0.0, 0.0, 1, BANNER)
    }

    fn side_open(cfg: &Config) -> f32 {
        side_open_w(cfg, 1.0) + cfg.window.bottom_margin as f32
    }

    #[test]
    fn a_side_box_stands_files_install_apps_from_the_edge_to_the_dock() {
        let cfg = config();
        let l = side_layout(Side::Left, side_open(&cfg), 30, 0.0);
        let x = |s: usize| l.sections[s].viewport.x;
        assert!(x(SECTION_FILES) < x(SECTION_INSTALL) && x(SECTION_INSTALL) < x(SECTION_APPS), "left: Files, Install, Apps");
        // The dock rides the box's far side: right of Apps.
        let apps = l.sections[SECTION_APPS].viewport;
        assert!(l.dock_slots.iter().all(|s| s.x >= apps.x + apps.w), "the dock after Apps");
        // Every title on one line; the pages go down; Apps 3 across, 18 a page.
        assert!(l.sections.iter().all(|s| (s.title_pos.1 - l.sections[0].title_pos.1).abs() < 0.01 && s.vertical));
        assert_eq!((l.sections[SECTION_APPS].cols, l.sections[SECTION_APPS].cap), (3, 18));
        assert_eq!((l.sections[SECTION_FILES].cols, l.sections[SECTION_FILES].cap), (1, 6));
        // Search under the sections.
        assert!(l.search_box.y > apps.y + apps.h);
        // On the right: mirrored, Apps by the dock.
        let r = side_layout(Side::Right, side_open(&cfg), 30, 0.0);
        let rx = |s: usize| r.sections[s].viewport.x;
        assert!(rx(SECTION_APPS) < rx(SECTION_INSTALL) && rx(SECTION_INSTALL) < rx(SECTION_FILES));
        assert!(r.dock_slots.iter().all(|s| s.x + s.w <= rx(SECTION_APPS)), "the dock before Apps");
        assert!((r.card_x + r.card_w - (SCREEN.0 - cfg.window.bottom_margin as f32)).abs() < 0.01, "a gap off the right edge");
    }

    #[test]
    fn the_dock_bounce_lifts_every_icon_off_its_edge() {
        let cfg = config();
        let docked = (cfg.window.input_bar_height + cfg.window.bottom_margin) as f32;
        let (es, order) = (entries(9), idslots(9));
        let icons = |side: Side, hop: f32| {
            let l = side_layout(side, docked, 0, 0.0);
            let s = scene(
                &cfg,
                1.0,
                &l,
                &es,
                &vis(9),
                SCREEN,
                &FrameInput { alpha: 1.0, stretch: 1.0, dock_order: &order, dock_hop: hop, ..Default::default() },
            );
            s.icons.iter().map(|i| i.rect).collect::<Vec<_>>()
        };
        let (rest, up) = (icons(Side::Bottom, 0.0), icons(Side::Bottom, 12.0));
        assert_eq!(rest.len(), up.len());
        assert!(rest.iter().zip(&up).all(|(a, b)| (a.y - b.y - 12.0).abs() < 0.01 && a.x == b.x), "all up, together");
        let (rest, out) = (icons(Side::Left, 0.0), icons(Side::Left, 12.0));
        assert!(rest.iter().zip(&out).all(|(a, b)| (b.x - a.x - 12.0).abs() < 0.01 && a.y == b.y), "a left dock's away from the edge");
        let (rest, out) = (icons(Side::Right, 0.0), icons(Side::Right, 12.0));
        assert!(rest.iter().zip(&out).all(|(a, b)| (a.x - b.x - 12.0).abs() < 0.01), "a right dock's to the left");
    }

    #[test]
    fn a_resting_side_dock_is_a_column_live_to_the_edge() {
        let cfg = config();
        let docked = (cfg.window.input_bar_height + cfg.window.bottom_margin) as f32;
        let l = side_layout(Side::Left, docked, 0, 0.0);
        assert!((l.card_x - cfg.window.bottom_margin as f32).abs() < 0.01 && (l.card_w - cfg.window.input_bar_height as f32).abs() < 0.01);
        // Centred on its run (under the banner to the bottom), one slot under another.
        let (first, last) = (l.dock_slots[0], l.dock_slots[8]);
        assert!(((first.y + last.y + last.h) / 2.0 - (BANNER + SCREEN.1) / 2.0).abs() < 0.01);
        assert!(l.dock_slots.windows(2).all(|p| (p[1].y - p[0].y - DOCK_SLOT).abs() < 0.01 && p[0].x == p[1].x));
        // The screen's very edge is live, row by row.
        let mid = |s: Rect| s.y + s.h / 2.0;
        assert_eq!(hit_test(&l, (1.0, mid(l.dock_slots[3])), false, &[]), Some(Hit::DockIcon(3)));
        assert_eq!(hit_test(&l, (60.0, mid(l.dock_slots[3])), false, &[]), None, "nothing past the plate");
        // Hidden: off the screen to the left.
        let hidden = side_layout(Side::Left, 0.0, 0, 0.0);
        assert!(hidden.card_x + hidden.card_w <= 0.01);
    }

    #[test]
    fn a_side_box_grows_with_a_long_dock() {
        let cfg = config();
        let open = side_open(&cfg);
        // 16 icons: a column longer than the six rows' box.
        let l = layout_on(Side::Left, &cfg, 1.0, SCREEN, open, 16, 0, 13, 0.0, &[], [60, 6, 9], [0.0; 3], false, (1.0, 1.0), None, 0.0, 0.0, 1, BANNER);
        let first = l.dock_slots[0];
        let last = l.dock_slots[15];
        assert!(l.card_top <= first.y && l.card_top + l.card_h >= last.y + last.h, "the box holds its dock");
        let apps = &l.sections[SECTION_APPS];
        assert!(apps.rows > SIDE_ROWS, "the sections gain rows: {}", apps.rows);
        assert_eq!(apps.cap, 3 * apps.rows);
        // A short dock keeps the six.
        let short = side_layout(Side::Left, open, 60, 0.0);
        assert_eq!(short.sections[SECTION_APPS].rows, SIDE_ROWS);
    }

    #[test]
    fn a_side_box_pages_down() {
        let cfg = config();
        let open = side_open(&cfg);
        let l = side_layout(Side::Left, open, 30, 0.0);
        let sec = &l.sections[SECTION_APPS];
        assert_eq!(sec.n_pages, 2);
        assert!((sec.page_len() - sec.viewport.h).abs() < 0.01);
        let cell = |l: &Layout, col: f32, row: f32| {
            let v = l.sections[SECTION_APPS].viewport;
            (v.x + (col + 0.5) * SIDE_CELL_W, v.y + (row + 0.5) * GRID_CELL_H)
        };
        let slots = idslots(30);
        assert_eq!(hit_test(&l, cell(&l, 2.0, 1.0), false, &slots), Some(Hit::GridCell(SECTION_APPS, 5)), "row-major, three across");
        // A page down: the same place is the second page's.
        let down = side_layout(Side::Left, open, 30, sec.viewport.h);
        assert_eq!(hit_test(&down, cell(&down, 0.0, 0.0), false, &slots), Some(Hit::GridCell(SECTION_APPS, 18)));
        assert!((scroll_to_reveal(sec, 20) - sec.viewport.h).abs() < 0.01);
        // The dock's icons, beside the box, by their rows.
        let s = l.dock_slots[2];
        assert_eq!(hit_test(&l, (s.x + s.w / 2.0, s.y + s.h / 2.0), false, &slots), Some(Hit::DockIcon(2)));
    }

    #[test]
    fn a_side_dock_overflows_down_its_column_and_opens_folders_beside_it() {
        let cfg = config();
        let docked = (cfg.window.input_bar_height + cfg.window.bottom_margin) as f32;
        // 40 pins and the trio: more than a 900 px column holds.
        let l = layout_on(Side::Left, &cfg, 1.0, SCREEN, docked, 43, 0, 40, 0.0, &[], [0; 3], [0.0; 3], false, (1.0, 1.0), None, 0.0, 0.0, 1, BANNER);
        let ov = l.dock_overflow.expect("the pins overflow");
        assert_eq!(ov.apps_slot, 40);
        let last = l.dock_slots.last().unwrap();
        assert!(l.dock_slots[0].y >= BANNER && last.y + last.h <= SCREEN.1, "the column stays under the banner, on the screen");
        // It runs the whole span before the Apps button eats a pin.
        assert!(l.card_top <= BANNER + 8.0 + 0.01 && l.card_top + l.card_h >= SCREEN.1 - 8.0 - 0.01, "{} .. {}", l.card_top, l.card_top + l.card_h);
        assert!((l.dock_slots[40].h - DOCK_SLOT).abs() < 0.01, "the trio keeps its slots");
        // A folder on it opens as a column beside the band, centred on its icon.
        let anchor = l.dock_slots[3];
        let (b, grid, cap) = col_box(l.dock_band, anchor, 5, (78.0, 70.0), Rect::new(0.0, 50.0, SCREEN.0, 800.0), 12.0, true);
        assert_eq!((grid, cap), ((1, 5), 11));
        assert!((b.x - (l.dock_band.x + l.dock_band.w + 12.0)).abs() < 0.01);
        assert!(b.y >= 50.0 && b.y + b.h <= 850.0);
        let (r, ..) = col_box(l.dock_band, anchor, 5, (78.0, 70.0), Rect::new(0.0, 50.0, SCREEN.0, 800.0), 12.0, false);
        assert!((r.x + r.w - (l.dock_band.x - 12.0)).abs() < 0.01, "on the right, before the band");
    }
}

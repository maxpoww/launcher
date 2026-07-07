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

/// One app-name label, centered under its icon.
#[derive(Debug, Clone)]
pub struct Label {
    pub text: String,
    /// Horizontal center and top edge of the text box.
    pub center: (f32, f32),
    /// Maximum text width; longer names are clipped.
    pub max_w: f32,
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
    /// Unclipped fills: card background first, then dock hover.
    pub rects: Vec<RectInst>,
    /// Dock icon quads (unclipped; they live in the card's top sliver).
    pub icons: Vec<IconInst>,
    /// App grid, present once the card is risen past the dock band.
    pub grid: Option<GridContent>,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    DockIcon(usize),
    GridCell(usize),
}

/// Fixed layout metrics (logical px). Config-independent for now; can
/// move into `[theme]` if tuning is wanted.
const DOCK_ICON: f32 = 36.0;
const DOCK_SLOT: f32 = 52.0;
const DOCK_PAD_X: f32 = 20.0;
const GRID_CELL_W: f32 = 104.0;
const GRID_CELL_H: f32 = 96.0;
const GRID_ICON: f32 = 48.0;
const GRID_PAD_X: f32 = 14.0;
const GRID_TOP_GAP: f32 = 8.0;
const GRID_BOTTOM_PAD: f32 = 10.0;
/// Text metrics the labels are laid out for (matches the renderer's
/// glyphon metrics).
pub const LABEL_FONT_PX: f32 = 12.0;
pub const LABEL_LINE_PX: f32 = 16.0;

/// Geometry shared by scene assembly and hit-testing.
#[derive(Debug)]
pub struct Layout {
    /// Card top edge in surface coordinates for the current extent.
    pub card_top: f32,
    /// Dock slot rects, one per shown dock icon (entry index == slot
    /// index: the dock shows the first N entries).
    pub dock_slots: Vec<Rect>,
    /// Grid viewport (may lie below the surface while docked).
    pub viewport: Rect,
    /// Left edge of the first grid column (columns are centered).
    pub grid_x0: f32,
    /// Number of grid columns (>= 1).
    pub cols: usize,
    /// Current scroll offset actually applied (clamped).
    pub scroll: f32,
    /// Total number of grid cells (all entries).
    pub cells: usize,
}

/// Compute the layout for the current animation state.
///
/// `surface` is the full surface size, `extent` the card's rise, `n`
/// the number of app entries, `scroll` the unclamped grid offset.
pub fn layout(config: &Config, surface: (f32, f32), extent: f32, n: usize, scroll: f32) -> Layout {
    let (w, h) = surface;
    let card_top = h - extent;
    let dock_h = config.window.input_bar_height as f32;
    let card_h = h - config.window.bottom_margin as f32;

    let max_slots = (((w - 2.0 * DOCK_PAD_X) / DOCK_SLOT).floor() as usize).max(1);
    let n_dock = n.min(max_slots);
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

    let grid_top = card_top + dock_h + GRID_TOP_GAP;
    let grid_bottom = (card_top + card_h - GRID_BOTTOM_PAD).min(h);
    let inner_w = (w - 2.0 * GRID_PAD_X).max(GRID_CELL_W);
    let cols = ((inner_w / GRID_CELL_W).floor() as usize).max(1);
    let grid_x0 = (w - cols as f32 * GRID_CELL_W) / 2.0;
    let viewport = Rect::new(
        grid_x0,
        grid_top,
        cols as f32 * GRID_CELL_W,
        (grid_bottom - grid_top).max(0.0),
    );
    let rows = n.div_ceil(cols);
    let max_scroll = (rows as f32 * GRID_CELL_H - viewport.h).max(0.0);

    Layout {
        card_top,
        dock_slots,
        viewport,
        grid_x0,
        cols,
        scroll: scroll.clamp(0.0, max_scroll),
        cells: n,
    }
}

/// Which item (if any) the pointer is over.
pub fn hit_test(layout: &Layout, pos: (f32, f32)) -> Option<Hit> {
    for (i, slot) in layout.dock_slots.iter().enumerate() {
        if slot.contains(pos) {
            return Some(Hit::DockIcon(i));
        }
    }
    if layout.viewport.h > 0.0 && layout.viewport.contains(pos) {
        let col = ((pos.0 - layout.grid_x0) / GRID_CELL_W).floor();
        let row = ((pos.1 - layout.viewport.y + layout.scroll) / GRID_CELL_H).floor();
        if col >= 0.0 && (col as usize) < layout.cols && row >= 0.0 {
            let i = row as usize * layout.cols + col as usize;
            if i < layout.cells {
                return Some(Hit::GridCell(i));
            }
        }
    }
    None
}

/// Assemble the draw scene for one frame.
pub fn scene(
    config: &Config,
    layout: &Layout,
    entries: &[AppEntry],
    hover: Option<Hit>,
    alpha: f32,
    surface: (f32, f32),
) -> Scene {
    let (w, h) = surface;
    let card_h = h - config.window.bottom_margin as f32;
    let mut scene = Scene {
        alpha,
        ..Default::default()
    };

    // Card background.
    scene.rects.push(RectInst {
        rect: Rect::new(0.0, layout.card_top, w, card_h),
        radius: config.theme.corner_radius,
        color: config.theme.background_rgba(),
    });

    // Dock row: hover highlight then icons.
    if let Some(Hit::DockIcon(i)) = hover {
        if let Some(slot) = layout.dock_slots.get(i) {
            scene.rects.push(RectInst {
                rect: *slot,
                radius: 12.0,
                color: config.theme.highlight_rgba(),
            });
        }
    }
    for (i, slot) in layout.dock_slots.iter().enumerate() {
        let inset = (DOCK_SLOT - DOCK_ICON) / 2.0;
        scene.icons.push(IconInst {
            rect: Rect::new(
                slot.x + inset,
                slot.y + (slot.h - DOCK_ICON).max(0.0) / 2.0,
                DOCK_ICON,
                DOCK_ICON.min(slot.h),
            ),
            layer: i as u32,
        });
    }

    // Grid cells, once there is any viewport to draw into.
    if layout.viewport.h > 1.0 && !entries.is_empty() {
        let mut grid = GridContent {
            clip: layout.viewport,
            ..Default::default()
        };
        let first_row = (layout.scroll / GRID_CELL_H).floor().max(0.0) as usize;
        let last_row = ((layout.scroll + layout.viewport.h) / GRID_CELL_H).ceil() as usize;
        for row in first_row..last_row {
            let y = layout.viewport.y + row as f32 * GRID_CELL_H - layout.scroll;
            for col in 0..layout.cols {
                let i = row * layout.cols + col;
                let Some(entry) = entries.get(i) else {
                    break;
                };
                let cell = Rect::new(
                    layout.grid_x0 + col as f32 * GRID_CELL_W,
                    y,
                    GRID_CELL_W,
                    GRID_CELL_H,
                );
                if hover == Some(Hit::GridCell(i)) {
                    grid.rects.push(RectInst {
                        rect: Rect::new(cell.x + 4.0, cell.y + 4.0, cell.w - 8.0, cell.h - 8.0),
                        radius: 14.0,
                        color: config.theme.highlight_rgba(),
                    });
                }
                let cx = cell.x + cell.w / 2.0;
                grid.icons.push(IconInst {
                    rect: Rect::new(cx - GRID_ICON / 2.0, cell.y + 12.0, GRID_ICON, GRID_ICON),
                    layer: i as u32,
                });
                grid.labels.push(Label {
                    text: entry.name.clone(),
                    center: (cx, cell.y + 12.0 + GRID_ICON + 8.0),
                    max_w: cell.w - 12.0,
                });
            }
        }
        scene.grid = Some(grid);
    }

    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Largest valid scroll offset for the current layout.
    fn max_scroll(layout: &Layout) -> f32 {
        let rows = layout.cells.div_ceil(layout.cols);
        (rows as f32 * GRID_CELL_H - layout.viewport.h).max(0.0)
    }

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

    fn config() -> Config {
        Config::default()
    }

    const SURFACE: (f32, f32) = (720.0, 572.0);

    #[test]
    fn docked_extent_shows_dock_row_only() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 48.0, 20, 0.0);
        assert!(!l.dock_slots.is_empty());
        // Viewport exists geometrically but lies below the surface
        // bottom, so no cells are visible while docked.
        let s = scene(&cfg, &l, &entries(20), None, 1.0, SURFACE);
        assert!(s.grid.is_none() || s.grid.as_ref().unwrap().clip.y >= SURFACE.1);
    }

    #[test]
    fn open_extent_shows_grid_and_clamps_scroll() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 40, 1e9);
        assert!(l.cols >= 2, "720px card should fit several columns");
        assert!(l.viewport.h > 100.0);
        assert!(l.scroll <= max_scroll(&l) + f32::EPSILON);
        let s = scene(&cfg, &l, &entries(40), None, 1.0, SURFACE);
        let grid = s.grid.expect("open card must show the grid");
        assert!(!grid.icons.is_empty());
        assert_eq!(grid.icons.len(), grid.labels.len());
    }

    #[test]
    fn hit_test_finds_dock_icon_and_cell() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 10, 0.0);
        let slot = l.dock_slots[0];
        assert_eq!(
            hit_test(&l, (slot.x + 1.0, slot.y + 1.0)),
            Some(Hit::DockIcon(0))
        );
        // Second cell of the first row.
        let cell1 = (
            l.grid_x0 + GRID_CELL_W * 1.5,
            l.viewport.y + GRID_CELL_H / 2.0,
        );
        assert_eq!(hit_test(&l, cell1), Some(Hit::GridCell(1)));
        assert_eq!(hit_test(&l, (1.0, 1.0)), None);
    }

    #[test]
    fn scrolled_hit_test_offsets_cells() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 100, GRID_CELL_H * 2.0);
        // Top-left visible cell is the first cell of row 2.
        let pos = (l.grid_x0 + 2.0, l.viewport.y + 2.0);
        assert_eq!(hit_test(&l, pos), Some(Hit::GridCell(2 * l.cols)));
    }

    #[test]
    fn partial_last_row_is_not_hit_beyond_entries() {
        let cfg = config();
        // 7 entries in >= 2 columns: the last row is partial.
        let l = layout(&cfg, SURFACE, 572.0, 7, 0.0);
        let last_row = 7 / l.cols;
        let beyond = 7 % l.cols; // first empty column of the last row
        if beyond > 0 {
            let pos = (
                l.grid_x0 + (beyond as f32 + 0.5) * GRID_CELL_W,
                l.viewport.y + (last_row as f32 + 0.5) * GRID_CELL_H,
            );
            assert_eq!(hit_test(&l, pos), None);
        }
    }
}

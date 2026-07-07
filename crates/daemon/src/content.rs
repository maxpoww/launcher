//! Card content layout: where the dock icon row and the app-list rows
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

/// One app-name label.
#[derive(Debug, Clone)]
pub struct Label {
    pub text: String,
    /// Top-left of the text box.
    pub pos: (f32, f32),
    /// Clip bounds for the glyph run.
    pub bounds: Rect,
}

/// Scrollable list content, clipped to `clip` by the renderer.
#[derive(Debug, Default)]
pub struct ListContent {
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
    /// List rows, present once the card is risen past the dock band.
    pub list: Option<ListContent>,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    DockIcon(usize),
    ListRow(usize),
}

/// Fixed layout metrics (logical px). Config-independent for now; can
/// move into `[theme]` if tuning is wanted.
const DOCK_ICON: f32 = 36.0;
const DOCK_SLOT: f32 = 52.0;
const DOCK_PAD_X: f32 = 20.0;
const LIST_ROW_H: f32 = 44.0;
const LIST_ICON: f32 = 26.0;
const LIST_PAD_X: f32 = 14.0;
const LIST_TOP_GAP: f32 = 6.0;
const LIST_BOTTOM_PAD: f32 = 10.0;
/// Text line height the labels are laid out for (matches the renderer's
/// glyphon metrics).
pub const LABEL_FONT_PX: f32 = 15.0;
pub const LABEL_LINE_PX: f32 = 20.0;

/// Geometry shared by scene assembly and hit-testing.
#[derive(Debug)]
pub struct Layout {
    /// Card top edge in surface coordinates for the current extent.
    pub card_top: f32,
    /// Dock slot rects, one per shown dock icon (entry index == slot
    /// index: the dock shows the first N entries).
    pub dock_slots: Vec<Rect>,
    /// List viewport (may have zero height while docked).
    pub viewport: Rect,
    /// Current scroll offset actually applied (clamped).
    pub scroll: f32,
    /// Number of list rows (all entries).
    pub rows: usize,
}

/// Compute the layout for the current animation state.
///
/// `surface` is the full surface size, `extent` the card's rise, `n`
/// the number of app entries, `scroll` the unclamped list offset.
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

    let list_top = card_top + dock_h + LIST_TOP_GAP;
    let list_bottom = (card_top + card_h - LIST_BOTTOM_PAD).min(h);
    let viewport = Rect::new(
        LIST_PAD_X,
        list_top,
        (w - 2.0 * LIST_PAD_X).max(0.0),
        (list_bottom - list_top).max(0.0),
    );
    let max_scroll = (n as f32 * LIST_ROW_H - viewport.h).max(0.0);

    Layout {
        card_top,
        dock_slots,
        viewport,
        scroll: scroll.clamp(0.0, max_scroll),
        rows: n,
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
        let row = ((pos.1 - layout.viewport.y + layout.scroll) / LIST_ROW_H).floor();
        if row >= 0.0 && (row as usize) < layout.rows {
            return Some(Hit::ListRow(row as usize));
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

    // List rows, once there is any viewport to draw into.
    if layout.viewport.h > 1.0 && !entries.is_empty() {
        let mut list = ListContent {
            clip: layout.viewport,
            ..Default::default()
        };
        let first = (layout.scroll / LIST_ROW_H).floor().max(0.0) as usize;
        let last = ((layout.scroll + layout.viewport.h) / LIST_ROW_H).ceil() as usize;
        for (i, entry) in entries
            .iter()
            .enumerate()
            .take(last.min(entries.len()))
            .skip(first)
        {
            let y = layout.viewport.y + i as f32 * LIST_ROW_H - layout.scroll;
            let row = Rect::new(layout.viewport.x, y, layout.viewport.w, LIST_ROW_H);
            if hover == Some(Hit::ListRow(i)) {
                list.rects.push(RectInst {
                    rect: Rect::new(row.x, row.y + 2.0, row.w, row.h - 4.0),
                    radius: 10.0,
                    color: config.theme.highlight_rgba(),
                });
            }
            list.icons.push(IconInst {
                rect: Rect::new(
                    row.x + 10.0,
                    y + (LIST_ROW_H - LIST_ICON) / 2.0,
                    LIST_ICON,
                    LIST_ICON,
                ),
                layer: i as u32,
            });
            list.labels.push(Label {
                text: entry.name.clone(),
                pos: (
                    row.x + 10.0 + LIST_ICON + 12.0,
                    y + (LIST_ROW_H - LABEL_LINE_PX) / 2.0,
                ),
                bounds: layout.viewport,
            });
        }
        scene.list = Some(list);
    }

    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Largest valid scroll offset for the current layout.
    fn max_scroll(layout: &Layout) -> f32 {
        (layout.rows as f32 * LIST_ROW_H - layout.viewport.h).max(0.0)
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
        // bottom, so no rows are visible while docked.
        let s = scene(&cfg, &l, &entries(20), None, 1.0, SURFACE);
        assert!(s.list.is_none() || s.list.as_ref().unwrap().clip.y >= SURFACE.1);
    }

    #[test]
    fn open_extent_shows_rows_and_clamps_scroll() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 40, 1e9);
        assert!(l.viewport.h > 100.0);
        assert!(l.scroll <= max_scroll(&l) + f32::EPSILON);
        let s = scene(&cfg, &l, &entries(40), None, 1.0, SURFACE);
        let list = s.list.expect("open card must show the list");
        assert!(!list.icons.is_empty());
        assert_eq!(list.icons.len(), list.labels.len());
    }

    #[test]
    fn hit_test_finds_dock_icon_and_row() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 10, 0.0);
        let slot = l.dock_slots[0];
        assert_eq!(
            hit_test(&l, (slot.x + 1.0, slot.y + 1.0)),
            Some(Hit::DockIcon(0))
        );
        let row1 = (l.viewport.x + 5.0, l.viewport.y + LIST_ROW_H * 1.5);
        assert_eq!(hit_test(&l, row1), Some(Hit::ListRow(1)));
        assert_eq!(hit_test(&l, (1.0, 1.0)), None);
    }

    #[test]
    fn scrolled_hit_test_offsets_rows() {
        let cfg = config();
        let l = layout(&cfg, SURFACE, 572.0, 40, LIST_ROW_H * 3.0);
        let top_row = (l.viewport.x + 5.0, l.viewport.y + 1.0);
        assert_eq!(hit_test(&l, top_row), Some(Hit::ListRow(3)));
    }
}

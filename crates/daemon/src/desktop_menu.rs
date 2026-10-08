//! The desktop's right-click menu (the mockup at `~/desktop-menu-mockup`,
//! Max's "build the menu", 2026-10-07): a small panel of the dock's box
//! material at the pointer, one band per row, no icons, no shortcut hints.
//! On an icon: Open · Open in terminal · Rename · then Move to Home · Move to bin · then Properties (on hover the
//! menu becomes the Properties box). On bare
//! wallpaper: New folder · Clean up.
//!
//! Pure here: what the rows are, where they sit, which one is under a
//! point, and how they are drawn into a [`Scene`]. The desktop (`desktop.rs`)
//! opens and closes it and acts on a row.

use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_BOLD};

/// What a row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Open,
    OpenTerminal,
    Rename,
    /// A little box of what there is to know about the item.
    Properties,
    /// Out of the desktop, into the home folder.
    MoveToHome,
    MoveToBin,
    NewFolder,
    CleanUp,
}

/// One row: an action, or a rule between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Item {
        label: &'static str,
        action: Action,
        /// Drawn in red: it throws something away.
        danger: bool,
    },
    Sep,
}

/// The menu's width.
pub(crate) const WIDTH: f32 = 192.0;
/// Padding inside the panel, around the rows.
const PAD: f32 = 6.0;
/// A row's height, and a rule's.
const ROW_H: f32 = 30.0;
const SEP_H: f32 = 11.0;
/// A row's hover band's corner; the panel's is the boxes' (`MenuPaint::radius`).
const ROW_RADIUS: f32 = 7.0;
/// Text: size and inset from the row's left edge.
const FONT_PX: f32 = 13.0;
const LINE_PX: f32 = 17.0;
const TEXT_X: f32 = 10.0;
/// How far from the pointer the panel's corner sits.
const GAP: f32 = 6.0;
/// A resting row's ink sits a little under full, so a hovered row (full,
/// bold) has somewhere to go — the boxes' rule.
const REST_INK: f32 = 0.86;
/// A rule between groups: the ink, faint.
const LINE_INK: f32 = 0.14;
/// The one colour of its own: a row that throws something away, and its
/// hover band.
const RED: [f32; 4] = [224.0 / 255.0, 82.0 / 255.0, 82.0 / 255.0, 1.0];
const HOVER_DANGER: [f32; 4] = [224.0 / 255.0, 82.0 / 255.0, 82.0 / 255.0, 0.16];

/// What the menu is painted with: the OPTIONS boxes' adaptive surface
/// (`App::box_surface_at`), read where the menu is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MenuPaint {
    /// The panel.
    pub fill: [f32; 4],
    /// Text that reads on it.
    pub ink: [f32; 4],
    /// A hovered row's band.
    pub wash: [f32; 4],
    /// The panel's corner.
    pub radius: f32,
}

/// The rows for a right-click on an item (`on_item`) or on bare wallpaper.
pub(crate) fn rows(on_item: bool) -> Vec<Row> {
    if on_item {
        vec![
            Row::Item { label: "Open", action: Action::Open, danger: false },
            Row::Item { label: "Open in terminal", action: Action::OpenTerminal, danger: false },
            Row::Sep,
            Row::Item { label: "Rename", action: Action::Rename, danger: false },
            Row::Sep,
            Row::Item { label: "Move to Home", action: Action::MoveToHome, danger: false },
            Row::Item { label: "Move to bin", action: Action::MoveToBin, danger: true },
            Row::Sep,
            // Last, and opened by HOVER: the menu turns into the box.
            Row::Item { label: "Properties", action: Action::Properties, danger: false },
        ]
    } else {
        vec![
            Row::Item { label: "New folder", action: Action::NewFolder, danger: false },
            Row::Sep,
            Row::Item { label: "Clean up", action: Action::CleanUp, danger: false },
        ]
    }
}

/// A menu that is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Menu {
    /// The item it was opened on, if any (the wallpaper's menu otherwise).
    pub item: Option<usize>,
    /// Where the pointer was: a new folder goes in that cell.
    pub at: (f32, f32),
    pub rows: Vec<Row>,
    /// The panel, placed so it stays on the surface.
    pub rect: Rect,
    /// The row under the pointer.
    pub hover: Option<usize>,
    /// The row the left button went down on.
    pub pressed: Option<usize>,
    /// Its entrance, 0..1: it fades in and settles down a few pixels.
    pub t: f32,
    /// Coming back from the Properties box: the shape it shrinks from.
    pub grow_from: Option<Rect>,
}

impl Menu {
    /// A menu at `at` on a `w`×`h` surface: hanging to the right and below
    /// the pointer, or flipped to stay on the surface.
    pub fn open(item: Option<usize>, at: (f32, f32), w: f32, h: f32) -> Self {
        let rows = rows(item.is_some());
        let height = PAD * 2.0 + rows.iter().map(|r| row_h(*r)).sum::<f32>();
        let x = if at.0 + GAP + WIDTH <= w { at.0 + GAP } else { (at.0 - GAP - WIDTH).max(0.0) };
        let y = if at.1 + GAP + height <= h { at.1 + GAP } else { (at.1 - GAP - height).max(0.0) };
        Self {
            item,
            at,
            rows,
            rect: Rect::new(x, y, WIDTH, height),
            hover: None,
            pressed: None,
            t: 0.0,
            grow_from: None,
        }
    }

    /// Each row's rectangle, in order (rules included).
    pub fn row_rects(&self) -> Vec<Rect> {
        let mut y = self.rect.y + PAD;
        self.rows
            .iter()
            .map(|r| {
                let h = row_h(*r);
                let rect = Rect::new(self.rect.x + PAD, y, WIDTH - 2.0 * PAD, h);
                y += h;
                rect
            })
            .collect()
    }

    /// The action row under `pos` (never a rule).
    pub fn hit(&self, pos: (f32, f32)) -> Option<usize> {
        self.row_rects()
            .iter()
            .enumerate()
            .find(|(i, r)| r.contains(pos) && matches!(self.rows[*i], Row::Item { .. }))
            .map(|(i, _)| i)
    }

    /// The action of row `i`.
    pub fn action(&self, i: usize) -> Option<Action> {
        match self.rows.get(i) {
            Some(Row::Item { action, .. }) => Some(*action),
            _ => None,
        }
    }

    /// Draw the menu over everything in `scene`: a grid of its own (grids
    /// paint after the icons), clipped to the panel.
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint) {
        let t = self.t.clamp(0.0, 1.0);
        // Its entrance: fading in while settling down from 4 px above — or,
        // back from the Properties box, that box's panel shrinking to ours.
        let lift = if self.grow_from.is_some() { 0.0 } else { (1.0 - t) * -4.0 };
        let fade = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * t];
        let ink_at = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a];
        let panel = match &self.grow_from {
            Some(from) => crate::desktop_props::lerp_rect(from, &self.rect, t),
            None => Rect::new(self.rect.x, self.rect.y + lift, self.rect.w, self.rect.h),
        };
        let panel_fill = if self.grow_from.is_some() { paint.fill } else { fade(paint.fill) };
        let mut grid = GridContent {
            clip: panel,
            ..Default::default()
        };
        grid.rects.push(RectInst {
            rect: panel,
            radius: paint.radius,
            color: panel_fill,
            glass: 0.0,
            border: 0.0,
        });
        for (i, (row, rect)) in self.rows.iter().zip(self.row_rects()).enumerate() {
            let rect = Rect::new(rect.x, rect.y + lift, rect.w, rect.h);
            match row {
                Row::Sep => grid.rects.push(RectInst {
                    rect: Rect::new(rect.x + 2.0, rect.y + (rect.h - 1.0) / 2.0, rect.w - 4.0, 1.0),
                    radius: 0.0,
                    color: fade(ink_at(LINE_INK)),
                    glass: 0.0,
                    border: 0.0,
                }),
                Row::Item { label, danger, .. } => {
                    let hot = self.hover == Some(i);
                    if hot {
                        grid.rects.push(RectInst {
                            rect,
                            radius: ROW_RADIUS,
                            color: fade(if *danger { HOVER_DANGER } else { paint.wash }),
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    // Hover is weight and full strength, as in every box's
                    // list: the colour itself does not move.
                    let color = if *danger {
                        RED
                    } else if hot {
                        ink_at(1.0)
                    } else {
                        ink_at(REST_INK)
                    };
                    grid.labels.push(Label {
                        text: (*label).to_owned(),
                        pos: (rect.x + TEXT_X, rect.y + (rect.h - LINE_PX) / 2.0),
                        max_w: rect.w - 2.0 * TEXT_X,
                        font_px: FONT_PX,
                        line_px: LINE_PX,
                        centered: false,
                        dim: false,
                        cache: true,
                        clip: Some(panel),
                        family: hot.then_some(FONT_BOLD),
                        color: Some(fade(color)),
                    });
                }
            }
        }
        scene.grids.push(grid);
    }
}

fn row_h(row: Row) -> f32 {
    match row {
        Row::Item { .. } => ROW_H,
        Row::Sep => SEP_H,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_menu_hangs_right_and_below_the_pointer_and_flips_at_the_edges() {
        let m = Menu::open(Some(3), (100.0, 100.0), 1000.0, 800.0);
        assert_eq!((m.rect.x, m.rect.y), (106.0, 106.0));
        assert_eq!(m.rect.w, WIDTH);
        // 6 rows + 3 rules, inside the padding.
        assert_eq!(m.rect.h, PAD * 2.0 + 6.0 * ROW_H + 3.0 * SEP_H);
        // Near the right and bottom edges it opens to the left and above.
        let m = Menu::open(Some(3), (950.0, 780.0), 1000.0, 800.0);
        assert_eq!(m.rect.x, 950.0 - GAP - WIDTH);
        assert!(m.rect.y + m.rect.h <= 780.0 - GAP + 0.01);
        // The wallpaper's menu is the short one.
        let m = Menu::open(None, (10.0, 10.0), 1000.0, 800.0);
        assert_eq!(m.rows.len(), 3);
        assert_eq!(m.action(0), Some(Action::NewFolder));
        assert_eq!(m.action(1), None, "a rule is not an action");
        assert_eq!(m.action(2), Some(Action::CleanUp));
    }

    #[test]
    fn hit_finds_rows_but_never_rules_or_the_padding() {
        let m = Menu::open(Some(0), (0.0, 0.0), 1000.0, 800.0);
        let rows = m.row_rects();
        let mid = |r: &Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(m.hit(mid(&rows[0])), Some(0));
        assert_eq!(m.action(0), Some(Action::Open));
        assert_eq!(m.hit(mid(&rows[2])), None, "the rule");
        assert_eq!(m.hit(mid(&rows[4])), None, "the second rule");
        assert_eq!(m.hit(mid(&rows[5])), Some(5));
        assert_eq!(m.action(5), Some(Action::MoveToHome));
        assert_eq!(m.hit(mid(&rows[6])), Some(6));
        assert_eq!(m.action(6), Some(Action::MoveToBin));
        assert_eq!(m.hit(mid(&rows[7])), None, "the third rule");
        assert_eq!(m.action(8), Some(Action::Properties), "last");
        assert_eq!(m.hit((m.rect.x + 1.0, m.rect.y + 1.0)), None, "the padding");
        assert_eq!(m.hit((5000.0, 5000.0)), None);
    }

    const PAINT: MenuPaint = MenuPaint {
        fill: [0.1, 0.1, 0.12, 0.9],
        ink: [0.9, 0.9, 0.9, 1.0],
        wash: [1.0, 1.0, 1.0, 0.1],
        radius: 10.0,
    };

    #[test]
    fn the_drawn_menu_is_one_grid_with_a_panel_a_hover_band_and_its_labels() {
        let mut m = Menu::open(Some(0), (0.0, 0.0), 1000.0, 800.0);
        m.t = 1.0;
        m.hover = Some(6);
        let mut scene = Scene::default();
        m.push(&mut scene, &PAINT);
        assert_eq!(scene.grids.len(), 1);
        let g = &scene.grids[0];
        assert_eq!(g.clip, m.rect);
        // The panel, two rules, one hover band.
        assert_eq!(g.rects.len(), 5);
        assert_eq!(g.rects[0].rect, m.rect);
        assert_eq!(g.rects[0].color, PAINT.fill, "the boxes' fill");
        assert_eq!(g.rects[0].radius, PAINT.radius);
        assert_eq!(g.rects[3].color, HOVER_DANGER, "the bin's band is red");
        assert_eq!(g.labels.len(), 6);
        assert_eq!(g.labels[3].text, "Move to Home");
        assert_eq!(g.labels[4].text, "Move to bin");
        assert_eq!(g.labels[4].color, Some(RED));
        assert_eq!(g.labels[4].family, Some(FONT_BOLD), "hovered: bold");
        assert_eq!(g.labels[5].text, "Properties");
        assert_eq!(g.labels[0].family, None);
        assert!((g.labels[0].color.unwrap()[3] - REST_INK).abs() < 1e-5, "resting: the ink, a little under full");
        // Half-way in, everything is half as strong and 2 px above its place.
        m.t = 0.5;
        let mut scene = Scene::default();
        m.push(&mut scene, &PAINT);
        let g = &scene.grids[0];
        assert!((g.rects[0].color[3] - PAINT.fill[3] * 0.5).abs() < 1e-5);
        assert!((g.rects[0].rect.y - (m.rect.y - 2.0)).abs() < 1e-5);
    }
}

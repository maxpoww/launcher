//! How the card is DRAWN: one frame as a scene, and the boxes its items
//! landed in (what the pointer is tested against).

use std::collections::HashMap;

use super::model::{Item, Kind};
use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene, ShadowInst, NO_PLATE};

pub(super) const RADIUS: f32 = 10.0;

/// The list: its padding, the gap between items, an item's own padding
/// and corner.
pub(super) const LIST_PAD: f32 = 12.0;

pub(super) const GAP: f32 = 8.0;

pub(super) const TILE_PAD_X: f32 = 10.0;

pub(super) const TILE_PAD_Y: f32 = 9.0;

pub(super) const TILE_RADIUS: f32 = 8.0;

pub(super) const TEXT_PX: f32 = 12.0;

pub(super) const TEXT_LINE: f32 = 17.0;

/// The small capital word over a file, a folder or a picture.
pub(super) const KIND_PX: f32 = 10.0;

pub(super) const KIND_LINE: f32 = 16.0;

/// A picture's box, and the air under it.
pub(super) const PIC_H: f32 = 120.0;

pub(super) const PIC_GAP: f32 = 6.0;

pub(super) const PIC_RADIUS: f32 = 6.0;

/// The × that takes an item off, at the item's top-right corner.
pub(super) const CLOSE: f32 = 22.0;

pub(super) const CLOSE_INSET: f32 = 4.0;

/// The mockup's orange: the kind word, and the rim while a drag is over.
pub(super) const ACCENT: [f32; 3] = [0.910, 0.576, 0.353];

pub(super) const DANGER: [f32; 3] = [0.878, 0.322, 0.322];

/// An item's box as last drawn, in surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tile {
    pub id: u64,
    pub rect: Rect,
}

impl Tile {
    /// Where its × is.
    pub(super) fn close(&self) -> Rect {
        Rect::new(
            self.rect.x + self.rect.w - CLOSE - CLOSE_INSET,
            self.rect.y + CLOSE_INSET,
            CLOSE,
            CLOSE,
        )
    }
}

/// How tall an item with `lines` lines of text is.
pub(crate) fn tile_height(kind: Kind, lines: usize) -> f32 {
    let text = lines.max(1) as f32 * TEXT_LINE;
    2.0 * TILE_PAD_Y
        + match kind {
            Kind::Text => text,
            Kind::Image => KIND_LINE + PIC_H + PIC_GAP + text,
            Kind::File | Kind::Folder => KIND_LINE + text,
        }
}

/// The colours a card is drawn in: the box's fill and ink (the OPTIONS
/// boxes' own, read on the card's side of the screen).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Paint {
    pub fill: [f32; 4],
    pub ink: [f32; 4],
}

impl Paint {
    /// A light box (its ink is dark): the insets darken less, the rims are
    /// dark instead of light.
    fn bright(&self) -> bool {
        0.2126 * self.ink[0] + 0.7152 * self.ink[1] + 0.0722 * self.ink[2] < 0.5
    }

    fn ink_at(&self, a: f32) -> [f32; 4] {
        [self.ink[0], self.ink[1], self.ink[2], self.ink[3] * a]
    }
}

/// Everything one frame of the card is drawn from.
pub(crate) struct View<'a> {
    pub rect: Rect,
    pub shown: f32,
    pub items: &'a [Item],
    pub lines: &'a HashMap<u64, Vec<String>>,
    pub scroll: f32,
    /// The item under the pointer, and whether the pointer is on its ×.
    pub hover: Option<(u64, bool)>,
    pub dnd_over: bool,
    /// The pictures that have arrived: path → texture layer.
    pub slots: &'a HashMap<String, u32>,
    pub paint: Paint,
}

/// The card as a scene, the boxes its items were drawn in, and how far the
/// list can be scrolled. The unroll is a clip from the top: the card is
/// whole underneath and `shown` of its height is let through.
pub(crate) fn scene(view: &View) -> (Scene, Vec<Tile>, f32) {
    let mut scene = Scene {
        alpha: 1.0,
        ..Default::default()
    };
    let rect = view.rect;
    let shown = view.shown.clamp(0.0, 1.0);
    if shown <= 0.0 || rect.h <= 0.0 {
        return (scene, Vec::new(), 0.0);
    }
    let paint = &view.paint;
    let bright = paint.bright();
    let window = Rect::new(rect.x, rect.y, rect.w, (rect.h * shown).round());

    scene.shadows.push(ShadowInst {
        rect: window,
        radius: RADIUS,
        blur: 26.0,
        color: [0.0, 0.0, 0.0, shown],
        edges: [0.18, 0.42, 0.34, 0.34],
    });

    let mut panel = GridContent {
        clip: window,
        ..Default::default()
    };
    panel.rects.push(RectInst {
        rect,
        radius: RADIUS,
        color: paint.fill,
        glass: 0.0,
        border: 0.0,
    });
    let rim = if view.dnd_over {
        [ACCENT[0], ACCENT[1], ACCENT[2], 0.7]
    } else if bright {
        [0.0, 0.0, 0.0, 0.12]
    } else {
        [1.0, 1.0, 1.0, 0.09]
    };
    panel.rects.push(RectInst {
        rect,
        radius: RADIUS,
        color: rim,
        glass: 0.0,
        border: if view.dnd_over { 1.5 } else { 1.0 },
    });
    scene.grids.push(panel);

    // The list, clipped a hair inside the card so nothing rides over its
    // rim, and to what the unroll has let through.
    let inner = Rect::new(rect.x + 1.0, rect.y + 1.0, rect.w - 2.0, rect.h - 2.0);
    let clip = Rect::new(
        inner.x,
        inner.y,
        inner.w,
        (window.y + window.h - inner.y).min(inner.h).max(0.0),
    );
    let mut list = GridContent {
        clip,
        ..Default::default()
    };
    let tile_w = rect.w - 2.0 * LIST_PAD;
    let text_w = tile_w - 2.0 * TILE_PAD_X;
    let heights: Vec<f32> = view
        .items
        .iter()
        .map(|it| tile_height(it.kind, view.lines.get(&it.id).map_or(1, Vec::len)))
        .collect();
    let total =
        2.0 * LIST_PAD + heights.iter().sum::<f32>() + GAP * heights.len().saturating_sub(1) as f32;
    let max_scroll = (total - rect.h).max(0.0);
    let scroll = view.scroll.clamp(0.0, max_scroll);

    let tile_fill = if bright {
        [0.0, 0.0, 0.0, 0.07]
    } else {
        [0.0, 0.0, 0.0, 0.28]
    };
    let tile_rim = if bright {
        [0.0, 0.0, 0.0, 0.10]
    } else {
        [1.0, 1.0, 1.0, 0.06]
    };
    let pic_fill = if bright {
        [0.0, 0.0, 0.0, 0.08]
    } else {
        [0.0, 0.0, 0.0, 0.30]
    };
    let mut tiles = Vec::with_capacity(view.items.len());
    let mut y = rect.y + LIST_PAD - scroll;
    for (item, h) in view.items.iter().zip(heights) {
        let tile = Tile {
            id: item.id,
            rect: Rect::new(rect.x + LIST_PAD, y.round(), tile_w, h),
        };
        y += h + GAP;
        tiles.push(tile);
        let t = tile.rect;
        if t.y > clip.y + clip.h || t.y + t.h < clip.y {
            continue;
        }
        list.rects.push(RectInst {
            rect: t,
            radius: TILE_RADIUS,
            color: tile_fill,
            glass: 0.0,
            border: 0.0,
        });
        list.rects.push(RectInst {
            rect: t,
            radius: TILE_RADIUS,
            color: tile_rim,
            glass: 0.0,
            border: 1.0,
        });
        let x = t.x + TILE_PAD_X;
        let mut line_y = t.y + TILE_PAD_Y;
        if let Some(word) = item.kind.word() {
            list.labels.push(Label {
                text: word.to_owned(),
                pos: (x, line_y),
                max_w: text_w,
                font_px: KIND_PX,
                line_px: KIND_LINE,
                centered: false,
                dim: false,
                cache: true,
                clip: Some(clip),
                family: None,
                color: Some([ACCENT[0], ACCENT[1], ACCENT[2], 0.85]),
            });
            line_y += KIND_LINE;
        }
        if item.kind == Kind::Image {
            let frame = Rect::new(x, line_y, text_w, PIC_H);
            list.rects.push(RectInst {
                rect: frame,
                radius: PIC_RADIUS,
                color: pic_fill,
                glass: 0.0,
                border: 0.0,
            });
            let layer = item.path.as_ref().and_then(|p| view.slots.get(p));
            if let Some(&layer) = layer {
                // The picture is kept whole on a square: the square is
                // sized so the picture itself fills the frame's height, or
                // its width where it is wider than that allows.
                let side = if item.aspect > 1.0 {
                    (PIC_H * item.aspect).min(frame.w)
                } else {
                    PIC_H
                };
                list.icons.push(IconInst {
                    rect: Rect::new(
                        // (Not rounded: the card's own edge is on a screen pixel,
                        // and a picture rounded apart from it would jitter
                        // against the card as it travels.)
                        frame.x + (frame.w - side) / 2.0,
                        (frame.y + (frame.h - side) / 2.0).round(),
                        side,
                        side,
                    ),
                    layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate: NO_PLATE,
                });
            }
            line_y += PIC_H + PIC_GAP;
        }
        let family = (item.kind == Kind::Text).then_some(crate::options::NERD);
        for line in view.lines.get(&item.id).into_iter().flatten() {
            if !line.is_empty() {
                list.labels.push(Label {
                    text: line.clone(),
                    pos: (x, line_y),
                    max_w: text_w,
                    font_px: TEXT_PX,
                    line_px: TEXT_LINE,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family,
                    color: Some(paint.ink_at(0.92)),
                });
            }
            line_y += TEXT_LINE;
        }
        // The × shows on the item under the pointer only.
        if let Some((id, on_close)) = view.hover {
            if id == item.id {
                let close = tile.close();
                if on_close {
                    list.rects.push(RectInst {
                        rect: close,
                        radius: 6.0,
                        color: [DANGER[0], DANGER[1], DANGER[2], 0.2],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                list.labels.push(Label {
                    text: "×".to_owned(),
                    pos: (close.x + close.w / 2.0, close.y + 1.0),
                    max_w: close.w,
                    font_px: 15.0,
                    line_px: CLOSE - 2.0,
                    centered: true,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family: None,
                    color: Some(if on_close {
                        [DANGER[0], DANGER[1], DANGER[2], 1.0]
                    } else {
                        paint.ink_at(0.45)
                    }),
                });
            }
        }
    }
    scene.grids.push(list);
    (scene, tiles, max_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::WIDTH;

    fn text(id: u64, body: &str) -> Item {
        Item {
            id,
            kind: Kind::Text,
            body: body.to_owned(),
            path: None,
            aspect: 0.0,
            owned: false,
        }
    }

    fn file(id: u64, kind: Kind, path: &str) -> Item {
        Item {
            id,
            kind,
            body: path.rsplit('/').next().unwrap_or(path).to_owned(),
            path: Some(path.to_owned()),
            aspect: 1.5,
            owned: false,
        }
    }

    const PAINT: Paint = Paint {
        fill: [0.16, 0.14, 0.19, 1.0],
        ink: [0.95, 0.94, 0.96, 1.0],
    };

    #[test]
    fn the_scene_lists_items_top_down_and_scrolls_to_the_newest() {
        let items = vec![
            text(1, "a"),
            file(2, Kind::File, "/tmp/x.txt"),
            file(3, Kind::Image, "/tmp/p.png"),
        ];
        let lines: HashMap<u64, Vec<String>> = items
            .iter()
            .map(|it| (it.id, vec![it.body.clone()]))
            .collect();
        let slots = HashMap::from([("/tmp/p.png".to_owned(), 4u32)]);
        let rect = Rect::new(600.0, 60.0, WIDTH, 200.0);
        let view = |scroll: f32, shown: f32| View {
            rect,
            shown,
            items: &items,
            lines: &lines,
            scroll,
            hover: Some((2, true)),
            dnd_over: false,
            slots: &slots,
            paint: PAINT,
        };
        let (scene, tiles, max_scroll) = scene(&view(0.0, 1.0));
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].rect.y, 60.0 + LIST_PAD);
        assert!(tiles[1].rect.y > tiles[0].rect.y && tiles[2].rect.y > tiles[1].rect.y);
        // Taller than the card: it scrolls, by exactly the overflow.
        let total = 2.0 * LIST_PAD
            + tile_height(Kind::Text, 1)
            + tile_height(Kind::File, 1)
            + tile_height(Kind::Image, 1)
            + 2.0 * GAP;
        assert_eq!(max_scroll, total - 200.0);
        // The picture is drawn from its layer, the × on the hovered item.
        assert!(scene.grids[1].icons.iter().any(|i| i.layer == 4));
        assert!(scene.grids[1].labels.iter().any(|l| l.text == "×"));
        // Scrolled past the end shows the newest at the bottom edge.
        let (_, tiles, _) = super::scene(&view(f32::MAX, 1.0));
        let last = tiles[2].rect;
        assert!((last.y + last.h - (rect.y + rect.h - LIST_PAD)).abs() <= 1.0);
        // Rolled up: nothing at all.
        let (scene, tiles, _) = super::scene(&view(0.0, 0.0));
        assert!(scene.grids.is_empty() && tiles.is_empty());
    }

    #[test]
    fn the_unroll_lets_the_card_through_from_the_top() {
        let items = vec![text(1, "a")];
        let lines = HashMap::from([(1, vec!["a".to_owned()])]);
        let slots = HashMap::new();
        let rect = Rect::new(0.0, 0.0, WIDTH, 400.0);
        let (scene, _, _) = scene(&View {
            rect,
            shown: 0.25,
            items: &items,
            lines: &lines,
            scroll: 0.0,
            hover: None,
            dnd_over: true,
            slots: &slots,
            paint: PAINT,
        });
        assert_eq!(scene.grids[0].clip.h, 100.0);
        // The card under the clip is whole, and wears the drag's rim.
        assert_eq!(scene.grids[0].rects[0].rect.h, 400.0);
        assert_eq!(scene.grids[0].rects[1].color[..3], ACCENT);
    }
}

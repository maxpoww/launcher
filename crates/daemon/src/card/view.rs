//! How the card is DRAWN: one frame as a scene, and the boxes its items
//! landed in (what the pointer is tested against).

use std::collections::{HashMap, HashSet};

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
    /// How far down it is drawn from where it belongs: it has moved aside
    /// to open a place for something being dragged in (`View::shifts`).
    pub shift: f32,
}

/// Where something let go at height `y` on the card would land among
/// `tiles`: before the first one whose middle is below `y` — judged where
/// each BELONGS, not where it has moved aside to (or the opening would chase
/// the pointer).
pub(crate) fn insert_index(tiles: &[Tile], y: f32) -> usize {
    tiles
        .iter()
        .filter(|t| t.rect.y - t.shift + t.rect.h / 2.0 < y)
        .count()
}

/// The card's three pages: the session on it now, the sessions put away,
/// the pinned items (Max, 2026-10-09: *"i just have three buttons, [new]
/// [memory] [pinned]"*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Page {
    #[default]
    Session,
    Memory,
    Pinned,
}

/// The buttons along the card's foot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Foot {
    New,
    Memory,
    Pinned,
}

impl Foot {
    const ALL: [Foot; 3] = [Foot::New, Foot::Pinned, Foot::Memory];

    fn word(self) -> &'static str {
        match self {
            Foot::New => "New",
            Foot::Memory => "Memory",
            Foot::Pinned => "Pinned",
        }
    }

    /// The page this button shows (New shows none: it acts).
    pub(super) fn page(self) -> Option<Page> {
        match self {
            Foot::New => None,
            Foot::Memory => Some(Page::Memory),
            Foot::Pinned => Some(Page::Pinned),
        }
    }
}

/// What the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hover {
    Item(u64),
    /// An item's ×.
    Close(u64),
    /// An item's pin.
    Pin(u64),
    Foot(Foot),
}

impl Hover {
    fn item(self) -> Option<u64> {
        match self {
            Hover::Item(id) | Hover::Close(id) | Hover::Pin(id) => Some(id),
            Hover::Foot(_) => None,
        }
    }
}

/// The foot of the card: which page is up, and which of the items shown
/// are pinned (their pin stays lit).
pub(crate) struct FootView<'a> {
    pub page: Page,
    pub pinned: &'a HashSet<u64>,
    /// On Memory's page: the session this window is working with (its
    /// row is rimmed).
    pub current: Option<u64>,
}

/// The foot's height, and its buttons' own.
pub(super) const FOOT_H: f32 = 46.0;
const FOOT_BUTTON_H: f32 = 28.0;
/// The pin glyph (fa-thumb-tack, in the Nerd font).
const GLYPH_PIN: &str = "\u{f08d}";

/// The part of the card the list has: all of it but the foot.
pub(crate) fn list_rect(rect: Rect, foot: bool) -> Rect {
    let h = if foot {
        (rect.h - FOOT_H).max(0.0)
    } else {
        rect.h
    };
    Rect::new(rect.x, rect.y, rect.w, h)
}

/// Where the foot's three buttons are: side by side, the card's width.
pub(crate) fn foot_buttons(rect: Rect) -> [(Foot, Rect); 3] {
    let w = (rect.w - 2.0 * LIST_PAD - 2.0 * GAP) / 3.0;
    let y = rect.y + rect.h - FOOT_H + (FOOT_H - FOOT_BUTTON_H) / 2.0 - 2.0;
    Foot::ALL.map(|f| {
        let n = Foot::ALL.iter().position(|x| *x == f).unwrap_or(0) as f32;
        (
            f,
            Rect::new(
                (rect.x + LIST_PAD + n * (w + GAP)).round(),
                y.round(),
                w.round(),
                FOOT_BUTTON_H,
            ),
        )
    })
}

impl Tile {
    /// Where its pin is: beside the ×.
    pub(super) fn pin(&self) -> Rect {
        let close = self.close();
        Rect::new(close.x - CLOSE - 2.0, close.y, CLOSE, CLOSE)
    }

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
    /// What the pointer is on.
    pub hover: Option<Hover>,
    /// The foot with its three buttons (`None`: a card without one).
    pub foot: Option<FootView<'a>>,
    pub dnd_over: bool,
    /// An item not drawn at all: it is in the hand, over the card.
    pub hidden: Option<u64>,
    /// How far each item is moved down right now, by id, to open a place
    /// for what is being dragged in (eased by the caller).
    pub shifts: &'a HashMap<u64, f32>,
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
    let body = list_rect(rect, view.foot.is_some());
    let inner = Rect::new(body.x + 1.0, body.y + 1.0, body.w - 2.0, body.h - 2.0);
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
    let page = view.foot.as_ref().map_or(Page::Session, |f| f.page);
    let tile_w = rect.w - 2.0 * LIST_PAD;
    let text_w = tile_w - 2.0 * TILE_PAD_X;
    let shown_items: Vec<&Item> = view
        .items
        .iter()
        .filter(|it| Some(it.id) != view.hidden)
        .collect();
    let heights: Vec<f32> = shown_items
        .iter()
        .map(|it| tile_height(it.kind, view.lines.get(&it.id).map_or(1, Vec::len)))
        .collect();
    // (The opening for something dragged in is part of the list's length.)
    let opening = shown_items
        .iter()
        .filter_map(|it| view.shifts.get(&it.id))
        .fold(0.0f32, |a, s| a.max(*s));
    let total = 2.0 * LIST_PAD
        + heights.iter().sum::<f32>()
        + GAP * heights.len().saturating_sub(1) as f32
        + opening;
    let max_scroll = (total - body.h).max(0.0);
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
    let mut tiles = Vec::with_capacity(shown_items.len());
    let mut y = rect.y + LIST_PAD - scroll;
    for (item, h) in shown_items.iter().copied().zip(heights) {
        let shift = view.shifts.get(&item.id).copied().unwrap_or(0.0);
        let tile = Tile {
            id: item.id,
            rect: Rect::new(rect.x + LIST_PAD, (y + shift).round(), tile_w, h),
            shift,
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
        let mine = page == Page::Memory
            && view
                .foot
                .as_ref()
                .is_some_and(|f| f.current == Some(item.id));
        list.rects.push(RectInst {
            rect: t,
            radius: TILE_RADIUS,
            color: if mine {
                [ACCENT[0], ACCENT[1], ACCENT[2], 0.55]
            } else {
                tile_rim
            },
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
        for (n, line) in view.lines.get(&item.id).into_iter().flatten().enumerate() {
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
                    // (A session in memory: its date stands out, what is
                    // in it sits back.)
                    color: Some(match (page, n) {
                        (Page::Memory, 0) => [ACCENT[0], ACCENT[1], ACCENT[2], 0.95],
                        (Page::Memory, _) => paint.ink_at(0.62),
                        _ => paint.ink_at(0.92),
                    }),
                });
            }
            line_y += TEXT_LINE;
        }
        // The × shows on the item under the pointer only; the pin beside
        // it too, and on a pinned item all the time (lit).
        let over = view.hover.filter(|h| h.item() == Some(item.id));
        let mut button = |rect: Rect,
                          glyph: &str,
                          family: Option<&'static str>,
                          px: f32,
                          on: bool,
                          hot: bool,
                          lit: [f32; 3]| {
            if hot {
                list.rects.push(RectInst {
                    rect,
                    radius: 6.0,
                    color: [lit[0], lit[1], lit[2], 0.2],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            list.labels.push(Label {
                text: glyph.to_owned(),
                pos: (
                    rect.x + rect.w / 2.0,
                    rect.y + if family.is_some() { 3.0 } else { 1.0 },
                ),
                max_w: rect.w,
                font_px: px,
                line_px: CLOSE - 2.0,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(clip),
                family,
                color: Some(if on {
                    [lit[0], lit[1], lit[2], 1.0]
                } else {
                    paint.ink_at(0.45)
                }),
            });
        };
        if over.is_some() {
            button(
                tile.close(),
                "×",
                None,
                15.0,
                over == Some(Hover::Close(item.id)),
                over == Some(Hover::Close(item.id)),
                DANGER,
            );
        }
        if page == Page::Session {
            let pinned = view
                .foot
                .as_ref()
                .is_some_and(|f| f.pinned.contains(&item.id));
            if over.is_some() || pinned {
                let on = pinned || over == Some(Hover::Pin(item.id));
                button(
                    tile.pin(),
                    GLYPH_PIN,
                    Some(crate::options::NERD),
                    12.0,
                    on,
                    over == Some(Hover::Pin(item.id)),
                    ACCENT,
                );
            }
        }
    }
    scene.grids.push(list);

    // The foot: New · Memory · Pinned, the page that is up lit.
    if let Some(foot) = &view.foot {
        let mut bar = GridContent {
            clip: window,
            ..Default::default()
        };
        let line = if bright {
            [0.0, 0.0, 0.0, 0.10]
        } else {
            [1.0, 1.0, 1.0, 0.07]
        };
        bar.rects.push(RectInst {
            rect: Rect::new(
                rect.x + LIST_PAD,
                body.y + body.h,
                rect.w - 2.0 * LIST_PAD,
                1.0,
            ),
            radius: 0.0,
            color: line,
            glass: 0.0,
            border: 0.0,
        });
        for (which, r) in foot_buttons(rect) {
            let up = which.page() == Some(foot.page);
            let hot = view.hover == Some(Hover::Foot(which));
            bar.rects.push(RectInst {
                rect: r,
                radius: TILE_RADIUS,
                color: match (up, hot, bright) {
                    (true, _, _) => [ACCENT[0], ACCENT[1], ACCENT[2], 0.07],
                    (_, true, true) => [0.0, 0.0, 0.0, 0.14],
                    (_, true, false) => [1.0, 1.0, 1.0, 0.10],
                    (_, _, true) => [0.0, 0.0, 0.0, 0.07],
                    (_, _, false) => [0.0, 0.0, 0.0, 0.28],
                },
                glass: 0.0,
                border: 0.0,
            });
            bar.labels.push(Label {
                text: which.word().to_owned(),
                pos: (r.x + r.w / 2.0, r.y + (r.h - TEXT_LINE) / 2.0),
                max_w: r.w,
                font_px: TEXT_PX,
                line_px: TEXT_LINE,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(window),
                family: None,
                color: Some(if up {
                    [ACCENT[0], ACCENT[1], ACCENT[2], 1.0]
                } else {
                    paint.ink_at(if hot { 0.95 } else { 0.72 })
                }),
            });
        }
        scene.grids.push(bar);
    }
    (scene, tiles, max_scroll)
}

/// A picture being made on the CPU: `w`×`h` pixels of premultiplied RGBA,
/// in LINEAR light like every colour the renderer is given (the screen's
/// own encoding is put on at the end, `bytes`) — so the picture comes out
/// the very colours the card is drawn in.
pub(crate) struct Canvas {
    pub w: usize,
    pub h: usize,
    px: Vec<f32>,
}

impl Canvas {
    pub(super) fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![0.0; w * h * 4],
        }
    }

    /// The picture as the screen wants it: premultiplied RGBA bytes.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len());
        for p in self.px.chunks_exact(4) {
            let a = p[3].clamp(0.0, 1.0);
            for c in &p[..3] {
                let straight = if a > 0.0 {
                    (c / a).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                out.push((crate::options::linear_to_srgb(straight) * a * 255.0).round() as u8);
            }
            out.push((a * 255.0).round() as u8);
        }
        out
    }

    /// Lay `color` (linear, straight alpha, 0..=1 each) over the pixel at
    /// (x, y), thinned by `cover`.
    pub(super) fn blend(&mut self, x: i32, y: i32, color: [f32; 4], cover: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let a = (color[3] * cover).clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let at = (y as usize * self.w + x as usize) * 4;
        for (c, ink) in color.iter().enumerate().take(3) {
            self.px[at + c] = ink * a + self.px[at + c] * (1.0 - a);
        }
        self.px[at + 3] = a + self.px[at + 3] * (1.0 - a);
    }

    /// Fill a rounded rectangle (pixels), its edge softened; with `stroke`
    /// only a band of that width just inside the edge.
    pub(super) fn round_rect(
        &mut self,
        r: Rect,
        radius: f32,
        color: [f32; 4],
        stroke: Option<f32>,
    ) {
        let (x0, y0) = (r.x.floor() as i32, r.y.floor() as i32);
        let (x1, y1) = ((r.x + r.w).ceil() as i32, (r.y + r.h).ceil() as i32);
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0);
        for y in y0..y1 {
            for x in x0..x1 {
                // Signed distance to the rounded box's edge (inside < 0).
                let qx = ((x as f32 + 0.5) - cx).abs() - (r.w / 2.0 - radius);
                let qy = ((y as f32 + 0.5) - cy).abs() - (r.h / 2.0 - radius);
                let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                let mut cover = (0.5 - d).clamp(0.0, 1.0);
                if let Some(width) = stroke {
                    cover *= (d + width + 0.5).clamp(0.0, 1.0);
                }
                self.blend(x, y, color, cover);
            }
        }
    }

    /// Lay a square picture (`side`² premultiplied RGBA, the first level
    /// of a mip chain) into `to`, scaled by its nearest pixels.
    fn picture(&mut self, pixels: &[u8], side: usize, to: Rect, clip: Rect) {
        if pixels.len() < side * side * 4 || to.w <= 0.0 || to.h <= 0.0 {
            return;
        }
        let (x0, y0) = (
            to.x.max(clip.x).floor() as i32,
            to.y.max(clip.y).floor() as i32,
        );
        let x1 = (to.x + to.w).min(clip.x + clip.w).ceil() as i32;
        let y1 = (to.y + to.h).min(clip.y + clip.h).ceil() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let sx = (((x as f32 + 0.5 - to.x) / to.w) * side as f32) as usize;
                let sy = (((y as f32 + 0.5 - to.y) / to.h) * side as f32) as usize;
                if sx >= side || sy >= side {
                    continue;
                }
                let at = (sy * side + sx) * 4;
                let a = pixels[at + 3] as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                // (Premultiplied in the chain: back to straight for `blend`.)
                let straight =
                    |c: u8| crate::options::srgb_to_linear((c as f32 / 255.0 / a).min(1.0));
                self.blend(
                    x,
                    y,
                    [
                        straight(pixels[at]),
                        straight(pixels[at + 1]),
                        straight(pixels[at + 2]),
                        a,
                    ],
                    1.0,
                );
            }
        }
    }
}

/// How `tile_picture` has a line drawn: canvas, line, font px, family,
/// colour, top-left corner.
pub(crate) type DrawText<'a> =
    dyn FnMut(&mut Canvas, &str, f32, Option<&'static str>, [f32; 4], (f32, f32)) + 'a;

/// An item as a PICTURE of itself: what is carried under the pointer when
/// it is dragged (Max, 2026-10-08: *"when i grab a item it becomes
/// invisible. i want to see it all the time"*) — its box as the card draws
/// it, on a plate of the card's own colour, `width` logical px wide at
/// `scale` buffer pixels each. `thumb`: a picture item's pixels. Text is
/// drawn by `text(canvas, line, font_px, family, colour, (x, y))`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tile_picture(
    item: &Item,
    lines: &[String],
    width: f32,
    scale: f32,
    paint: &Paint,
    thumb: Option<&[u8]>,
    thumb_side: usize,
    text: &mut DrawText,
) -> Canvas {
    let bright = paint.bright();
    let height = tile_height(item.kind, lines.len());
    let s = |v: f32| v * scale;
    // (Whole logical pixels: the buffer is shown at 1/`scale` of its size.)
    let mut canvas = Canvas::new(s(width.round()) as usize, s(height.round()) as usize);
    let whole = Rect::new(0.0, 0.0, canvas.w as f32, canvas.h as f32);
    // The card's own colour underneath, then the item's inset and rim —
    // the list's own values, so it looks in the hand as it did in the list
    // (Max, 2026-10-08: *"i dont want the item to change color when im
    // draging it"*).
    canvas.round_rect(whole, s(TILE_RADIUS), paint.fill, None);
    let (inset, rim) = if bright {
        ([0.0, 0.0, 0.0, 0.07], [0.0, 0.0, 0.0, 0.10])
    } else {
        ([0.0, 0.0, 0.0, 0.28], [1.0, 1.0, 1.0, 0.06])
    };
    canvas.round_rect(whole, s(TILE_RADIUS), inset, None);
    canvas.round_rect(whole, s(TILE_RADIUS), rim, Some(s(1.0)));

    let x = TILE_PAD_X;
    let text_w = width - 2.0 * TILE_PAD_X;
    let mut y = TILE_PAD_Y;
    if let Some(word) = item.kind.word() {
        text(
            &mut canvas,
            word,
            s(KIND_PX),
            None,
            [ACCENT[0], ACCENT[1], ACCENT[2], 0.85],
            (s(x), s(y)),
        );
        y += KIND_LINE;
    }
    if item.kind == Kind::Image {
        let frame = Rect::new(s(x), s(y), s(text_w), s(PIC_H));
        let fill = if bright {
            [0.0, 0.0, 0.0, 0.08]
        } else {
            [0.0, 0.0, 0.0, 0.30]
        };
        canvas.round_rect(frame, s(PIC_RADIUS), fill, None);
        if let Some(pixels) = thumb {
            let side = if item.aspect > 1.0 {
                (s(PIC_H) * item.aspect).min(frame.w)
            } else {
                s(PIC_H)
            };
            let to = Rect::new(
                frame.x + (frame.w - side) / 2.0,
                frame.y + (frame.h - side) / 2.0,
                side,
                side,
            );
            canvas.picture(pixels, thumb_side, to, frame);
        }
        y += PIC_H + PIC_GAP;
    }
    let family = (item.kind == Kind::Text).then_some(crate::options::NERD);
    let ink = paint.ink_at(0.92);
    for line in lines {
        if !line.is_empty() {
            text(&mut canvas, line, s(TEXT_PX), family, ink, (s(x), s(y)));
        }
        y += TEXT_LINE;
    }
    canvas
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
            at: 0,
            from: None,
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
            at: 0,
            from: None,
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
        let shifts: HashMap<u64, f32> = HashMap::new();
        let rect = Rect::new(600.0, 60.0, WIDTH, 200.0);
        let view = |scroll: f32, shown: f32| View {
            rect,
            shown,
            items: &items,
            lines: &lines,
            scroll,
            hover: Some(Hover::Close(2)),
            foot: None,
            dnd_over: false,
            hidden: None,
            shifts: &shifts,
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
        let shifts: HashMap<u64, f32> = HashMap::new();
        let rect = Rect::new(0.0, 0.0, WIDTH, 400.0);
        let (scene, _, _) = scene(&View {
            rect,
            shown: 0.25,
            items: &items,
            lines: &lines,
            scroll: 0.0,
            hover: None,
            foot: None,
            dnd_over: true,
            hidden: None,
            shifts: &shifts,
            slots: &slots,
            paint: PAINT,
        });
        assert_eq!(scene.grids[0].clip.h, 100.0);
        // The card under the clip is whole, and wears the drag's rim.
        assert_eq!(scene.grids[0].rects[0].rect.h, 400.0);
        assert_eq!(scene.grids[0].rects[1].color[..3], ACCENT);
    }

    #[test]
    fn the_list_opens_a_place_where_a_drag_would_land() {
        let items = vec![text(1, "a"), text(2, "b"), text(3, "c")];
        let lines: HashMap<u64, Vec<String>> = items
            .iter()
            .map(|it| (it.id, vec![it.body.clone()]))
            .collect();
        let slots = HashMap::new();
        let rect = Rect::new(0.0, 0.0, WIDTH, 600.0);
        let view = |shifts: &HashMap<u64, f32>, hidden: Option<u64>| {
            scene(&View {
                rect,
                shown: 1.0,
                items: &items,
                lines: &lines,
                scroll: 0.0,
                hover: None,
                foot: None,
                dnd_over: true,
                hidden,
                shifts,
                slots: &slots,
                paint: PAINT,
            })
            .1
        };
        let none = HashMap::new();
        let rest = view(&none, None);
        let h = tile_height(Kind::Text, 1);
        // Above the first, between, and below the last.
        assert_eq!(insert_index(&rest, rest[0].rect.y - 5.0), 0);
        assert_eq!(insert_index(&rest, rest[0].rect.y + h + 2.0), 1);
        assert_eq!(insert_index(&rest, rest[1].rect.y + h * 0.9), 2);
        assert_eq!(insert_index(&rest, 590.0), 3);
        // The second and third have moved down 40 to open a place before
        // the second: they are DRAWN lower…
        let shifts = HashMap::from([(2u64, 40.0f32), (3, 40.0)]);
        let open = view(&shifts, None);
        assert_eq!(open[0].rect.y, rest[0].rect.y);
        assert_eq!(open[1].rect.y, rest[1].rect.y + 40.0);
        // …but a pointer in the opening still lands before the second: it
        // is judged where each belongs, so the opening does not run away.
        assert_eq!(insert_index(&open, rest[1].rect.y + 10.0), 1);
        // The item in the hand is not in the list while it is carried.
        let carried = view(&none, Some(2));
        assert_eq!(carried.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(carried[1].rect.y, rest[1].rect.y);
    }

    #[test]
    fn a_dragged_item_is_a_picture_of_its_own_box() {
        let item = text(1, "hello");
        let mut asked = Vec::new();
        let canvas = tile_picture(
            &item,
            &["hello".to_owned()],
            200.0,
            2.0,
            &PAINT,
            None,
            0,
            &mut |c, line, px, _, ink, at| {
                asked.push((line.to_owned(), px, at));
                c.blend(at.0 as i32, at.1 as i32, ink, 1.0);
            },
        );
        // Its box, at two buffer pixels a logical one.
        assert_eq!(canvas.w, 400);
        assert_eq!(
            canvas.h,
            (tile_height(Kind::Text, 1) * 2.0).round() as usize
        );
        assert_eq!(
            asked,
            [(
                "hello".to_owned(),
                TEXT_PX * 2.0,
                (TILE_PAD_X * 2.0, TILE_PAD_Y * 2.0)
            )]
        );
        // The middle is the plate (solid enough to be seen over anything),
        // the very corner is clear: the box is rounded.
        let bytes = canvas.bytes();
        let alpha = |x: usize, y: usize| bytes[(y * canvas.w + x) * 4 + 3];
        assert!(alpha(200, canvas.h / 2) > 240);
        assert_eq!(alpha(0, 0), 0);
    }

    #[test]
    fn the_foot_takes_its_height_from_the_list_and_holds_three_buttons() {
        let items: Vec<Item> = (1..=8).map(|n| text(n, "a")).collect();
        let lines: HashMap<u64, Vec<String>> = items
            .iter()
            .map(|it| (it.id, vec![it.body.clone()]))
            .collect();
        let (shifts, slots, pinned) = (HashMap::new(), HashMap::new(), HashSet::from([2u64]));
        let rect = Rect::new(600.0, 60.0, WIDTH, 200.0);
        let view = |foot: bool| View {
            rect,
            shown: 1.0,
            items: &items,
            lines: &lines,
            scroll: 0.0,
            hover: None,
            foot: foot.then_some(FootView {
                page: Page::Session,
                pinned: &pinned,
                current: None,
            }),
            dnd_over: false,
            hidden: None,
            shifts: &shifts,
            slots: &slots,
            paint: PAINT,
        };
        let (bare, _, scroll_bare) = scene(&view(false));
        let (with, _, scroll_foot) = scene(&view(true));
        // The list is shorter by the foot, so there is that much more to scroll.
        assert_eq!(scroll_foot, scroll_bare + FOOT_H);
        assert_eq!(with.grids.len(), bare.grids.len() + 1);
        // Three buttons, in the foot, side by side inside the card.
        let buttons = foot_buttons(rect);
        assert_eq!(
            buttons.map(|(f, _)| f),
            [Foot::New, Foot::Pinned, Foot::Memory]
        );
        for (_, r) in buttons {
            assert!(r.y >= rect.y + rect.h - FOOT_H && r.y + r.h <= rect.y + rect.h);
            assert!(r.x >= rect.x && r.x + r.w <= rect.x + rect.w);
        }
        assert!(buttons[0].1.x + buttons[0].1.w <= buttons[1].1.x);
        // A pinned item wears its pin without the pointer on it.
        let pins = |s: &Scene| {
            s.grids[1]
                .labels
                .iter()
                .filter(|l| l.text == GLYPH_PIN)
                .count()
        };
        assert_eq!((pins(&bare), pins(&with)), (0, 1));
    }
}

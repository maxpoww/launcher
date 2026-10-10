//! How the card is DRAWN: one frame as a scene, and the boxes its items
//! landed in (what the pointer is tested against).

use std::collections::{HashMap, HashSet};

use super::bar::{BarView, BoxBtn, Field};
use super::model::{Item, Kind, MAX_LINES};
use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene, ShadowInst, NO_PLATE};

pub(super) const RADIUS: f32 = 10.0;
/// How solid the card's ground is (1 = opaque).
pub(super) const AIR: f32 = 0.72;

/// The list: its padding, the gap between items, an item's own padding
/// and corner.
pub(super) const LIST_PAD: f32 = 12.0;

pub(super) const GAP: f32 = 8.0;

pub(super) const TILE_PAD_X: f32 = 10.0;

pub(super) const TILE_PAD_Y: f32 = 9.0;

pub(super) const TILE_RADIUS: f32 = RADIUS;

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
    /// The plain clipboard's history, reachable from the card.
    Clipboard,
    /// A word looked up (the offline dictionaries).
    Dictionary,
    /// Every emoji, the whole card of them: for when a few emoji are all
    /// that is wanted (they go straight into the window).
    Emoji,
}

impl Page {
    /// Its name, shown small under the head's buttons while it is up — the
    /// lit button is an arrow then, and says nothing of where one is. (An
    /// open session has its own name floating there instead. "Memory" is
    /// the whole thing; one of its lists is a SESSION — Max, 2026-10-10.)
    pub(crate) fn title(self) -> Option<&'static str> {
        match self {
            Page::Session => None,
            Page::Memory => Some("Sessions"),
            Page::Pinned => Some("Pinned"),
            Page::Clipboard => Some("Clipboard"),
            Page::Dictionary => Some("Dictionary"),
            Page::Emoji => Some("Emoji"),
        }
    }
}

/// The buttons along the card's HEAD (they were its foot until Max,
/// 2026-10-09: *"the buttons have to be on the top"* — the card is called
/// from the title bar, and they are then right under the pointer). From
/// the left (Max, 2026-10-10, settled on ~/terminal-mockup/card): the round
/// ones with an icon — emoji, pinned, the clipboard, the dictionary — then
/// MEMORY by name, at the right, with what the round ones leave. (The
/// mockup has two more round ones, voice-to-text first and the translator
/// before the dictionary: they come with their pages.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Foot {
    Memory,
    Pinned,
    Clipboard,
    Dictionary,
    Emoji,
}

/// The button of the page that is up is an ARROW POINTING UP: a click on it
/// goes back to the session (fa-arrow-up).
const GLYPH_UP: &str = "\u{f062}";

impl Foot {
    const ALL: [Foot; 5] = [
        Foot::Emoji,
        Foot::Pinned,
        Foot::Clipboard,
        Foot::Dictionary,
        Foot::Memory,
    ];

    /// Its icon (the round ones) or its word (Memory).
    fn word(self) -> &'static str {
        match self {
            Foot::Memory => "Memory",
            Foot::Pinned => GLYPH_PIN,
            Foot::Clipboard => "\u{f0ea}",  // fa-clipboard
            Foot::Dictionary => "\u{f02d}", // fa-book
            Foot::Emoji => "\u{f118}",      // fa-smile-o
        }
    }

    fn round(self) -> bool {
        !matches!(self, Foot::Memory)
    }

    /// The page this button shows.
    pub(super) fn page(self) -> Page {
        match self {
            Foot::Memory => Page::Memory,
            Foot::Pinned => Page::Pinned,
            Foot::Clipboard => Page::Clipboard,
            Foot::Dictionary => Page::Dictionary,
            Foot::Emoji => Page::Emoji,
        }
    }
}

/// What the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hover {
    Item(u64),
    /// An item's ×.
    Close(u64),
    /// An item's DISK (it was its pin): keep it somewhere — its menu.
    Pin(u64),
    /// A row of the disk's menu.
    Menu(usize),
    /// The hint, while it offers a way back ("Removed · Undo").
    Undo,
    /// A long text's "Show all" / "Show less".
    More(u64),
    /// An item's pencil: it is opened again to be changed.
    Edit(u64),
    Foot(Foot),
    /// The search, and the × of an open one.
    Seek,
    SeekClear,
    /// The input box (its words), and one of its buttons.
    Input,
    Btn(BoxBtn),
    /// New (Memory's page).
    New,
    /// The open memory's name.
    Name,
    /// A voice note's play button.
    Play(u64),
    /// One of the picker's emoji (its place among those shown), its
    /// search, and the rest of it.
    Emoji(usize),
    PickSeek,
    Tray,
    /// One of the picker's kinds (its place in `emoji_table::GROUPS`).
    Kind(usize),
}

impl Hover {
    fn item(self) -> Option<u64> {
        match self {
            Hover::Item(id)
            | Hover::Close(id)
            | Hover::Pin(id)
            | Hover::Play(id)
            | Hover::Edit(id)
            | Hover::More(id) => Some(id),
            _ => None,
        }
    }
}

/// The foot of the card: which page is up, and which of the items shown
/// are pinned (their pin stays lit).
pub(crate) struct FootView<'a> {
    pub page: Page,
    /// On Memory's page: the session this window is working with (its
    /// row is rimmed).
    pub current: Option<u64>,
    /// The search, the input box, the name (`bar.rs`).
    pub bar: BarView<'a>,
    /// The small line at the foot of each item, by id: when it is from
    /// (and, on the clipboard's page, which app).
    pub notes: &'a HashMap<u64, String>,
    /// The voice note being played, and for how many seconds now.
    pub playing: Option<(u64, f32)>,
    /// Something is being written IN THE LIST: a new thing, at its end
    /// (`Some(None)`), or the item of this id, in its own place.
    pub writing: Option<Option<u64>>,
    /// The page's name, when it is another than its own (Memory's while a
    /// session is being CHOSEN to keep something in).
    pub title: Option<&'a str>,
    /// A session is being chosen: its rows only choose — no × on them.
    pub choosing: bool,
    /// The disk's menu, when it is open.
    pub menu: Option<MenuView<'a>>,
    /// The parts already put in what is being written (over the writing).
    pub sealed: &'a [Item],
    /// The items that are OPEN: a click on one made of parts opened it —
    /// its voice notes show what they say, its pieces answer by themselves.
    pub wide: &'a HashSet<u64>,
    /// On Memory's page: the session whose × was clicked once (it asks to
    /// be clicked again), and the one being renamed with what is typed.
    pub arming: Option<u64>,
    pub renaming: Option<(u64, &'a str)>,
}

/// The id the list gives the tile of the box something is being written in
/// (`FootView::writing`): where that box is this frame.
pub(crate) const WRITER: u64 = u64::MAX - 7;
/// The disk (fa-floppy-o).
const GLYPH_DISK: &str = "\u{f0c7}";
/// The disk's menu: how wide it is, how tall a row, its inner margin.
pub(crate) const MENU_W: f32 = 150.0;
pub(crate) const MENU_ROW: f32 = 28.0;
pub(crate) const MENU_PAD: f32 = 4.0;

/// The disk's menu, open under an item's disk: where, its rows, the one
/// the pointer is on.
pub(crate) struct MenuView<'a> {
    pub rect: Rect,
    pub rows: &'a [&'static str],
    pub hot: Option<usize>,
}

/// The pencil (fa-pencil).
const GLYPH_EDIT: &str = "\u{f040}";

/// The foot's height, and its buttons' own.
pub(super) const FOOT_H: f32 = 46.0;
const FOOT_BUTTON_H: f32 = 28.0;
/// The pin glyph (fa-thumb-tack, in the Nerd font).
const GLYPH_PIN: &str = "\u{f08d}";

/// The line a page's title has under the head's buttons.
pub(super) const TITLE_H: f32 = 16.0;

/// How tall the head is with `page` up: its buttons, and the page's title
/// under them when it has one.
pub(crate) fn head_h(page: Page) -> f32 {
    FOOT_H + page.title().map_or(0.0, |_| TITLE_H)
}

/// The part of the card the list has: all of it but the head (`page`: the
/// one that is up; `None`, a card without a head) and what is at the
/// bottom (`bottom`: how tall that is).
pub(crate) fn list_rect(rect: Rect, page: Option<Page>, bottom: f32) -> Rect {
    let head = page.map_or(0.0, |p| head_h(p).min(rect.h));
    Rect::new(
        rect.x,
        rect.y + head,
        rect.w,
        (rect.h - head - bottom).max(0.0),
    )
}

/// Where the head's buttons are: the two named ones share what the three
/// round ones, at the right, leave.
pub(crate) fn foot_buttons(rect: Rect) -> [(Foot, Rect); 5] {
    let y = (rect.y + (FOOT_H - FOOT_BUTTON_H) / 2.0 + 2.0).round();
    let rounds = Foot::ALL.iter().filter(|f| f.round()).count() as f32;
    let names = Foot::ALL.len() as f32 - rounds;
    let round_w = rounds * (FOOT_BUTTON_H + GAP);
    let w = ((rect.w - 2.0 * LIST_PAD - round_w - (names - 1.0) * GAP) / names).round();
    let mut x = rect.x + LIST_PAD;
    Foot::ALL.map(|f| {
        let this = if f.round() { FOOT_BUTTON_H } else { w };
        let r = Rect::new(x.round(), y, this, FOOT_BUTTON_H);
        x += this + GAP;
        (f, r)
    })
}

/// The room an open memory's name takes at the top of its list.
const NAME_SPACE: f32 = 30.0;
/// A voice note's play button.
pub(super) const PLAY: f32 = 28.0;
/// A session's row on Memory's page: its name, and when its last thing was.
pub(super) const ROW_H: f32 = 38.0;
/// The line at the foot of an item (when it is from).
const NOTE_PX: f32 = 9.5;
const NOTE_LINE: f32 = 12.0;

/// Whether a text's lines are not all of it (its last line ends in "…":
/// `model::wrap` cut it at `MAX_LINES`), or are all of a long one (it was
/// opened): either way it has a "Show all" / "Show less" at its foot.
pub(crate) fn has_more(lines: &[String], wide: bool) -> bool {
    if wide {
        lines.len() > MAX_LINES
    } else {
        lines.len() >= MAX_LINES && lines.last().is_some_and(|l| l.ends_with('…'))
    }
}

impl Tile {
    /// Where a long text's "Show all" is: in the middle of its foot, on
    /// the line its time is on.
    pub(super) fn more(&self) -> Rect {
        Rect::new(
            self.rect.x + self.rect.w / 2.0 - 45.0,
            self.rect.y + self.rect.h - NOTE_LINE - 8.0,
            90.0,
            NOTE_LINE + 6.0,
        )
    }

    /// Where its pencil is: left of the pin where there is one (`pins`),
    /// else in the pin's place, beside the ×.
    pub(super) fn edit(&self, pins: bool) -> Rect {
        let pin = self.pin();
        if pins {
            Rect::new(pin.x - CLOSE - 2.0, pin.y, CLOSE, CLOSE)
        } else {
            pin
        }
    }

    /// Where its pin is: beside the ×.
    pub(super) fn pin(&self) -> Rect {
        let close = self.close();
        Rect::new(close.x - CLOSE - 2.0, close.y, CLOSE, CLOSE)
    }

    /// Where a voice note's play button is.
    pub(super) fn play(&self) -> Rect {
        Rect::new(
            self.rect.x + TILE_PAD_X,
            self.rect.y + TILE_PAD_Y - 1.0,
            PLAY,
            PLAY,
        )
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
pub(crate) fn tile_height(kind: Kind, lines: usize, zoom: f32) -> f32 {
    // The items' own sizes, at the card's zoom.
    let (_text_px, text_line) = (TEXT_PX * zoom, (TEXT_LINE * zoom).round());
    let (_kind_px, kind_line) = (KIND_PX * zoom, (KIND_LINE * zoom).round());
    let pic_h = (PIC_H * zoom).round();
    let text = lines.max(1) as f32 * text_line;
    2.0 * TILE_PAD_Y - 3.0
        + NOTE_LINE
        + match kind {
            Kind::Text => text,
            Kind::Image => kind_line + pic_h + PIC_GAP + text,
            Kind::File | Kind::Folder => kind_line + text,
            // (A voice note: its title beside the play button, the line
            // with its two times; what is said in it under them, when shown.)
            Kind::Voice => {
                VOICE_HEAD
                    + if lines > 0 {
                        4.0 + lines as f32 * text_line
                    } else {
                        0.0
                    }
            }
        }
}

/// A voice note's own rows: the title beside the play button, and the line
/// with its two times ("00:00 ——|—— 01:13").
const VOICE_HEAD: f32 = 40.0;
/// What a voice note with no title of its own is called (its body starts
/// so: "Voice note · 0:07").
pub(crate) const VOICE_NAME: &str = "Voice note";
/// What stands in for what a voice note says while that is being written out.
pub(crate) const WRITING_OUT: &str = "Writing out what it says…";

/// A file as a part of an item: a small square of it, its name under.
pub(crate) const THUMB: f32 = 64.0;
const THUMB_NAME: f32 = 14.0;
const THUMB_GAP: f32 = 8.0;
/// A voice note as a part of an item: a row with play, a line, its length.
const VOICE_ROW: f32 = 30.0;
/// Between two pieces of an item.
const PIECE_GAP: f32 = 7.0;

/// One PIECE of an item made of several things, as it is laid out: some
/// words, a row of files (those put in one after the other stand side by
/// side), a voice note. The numbers are places in the item's `parts`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PieceKind {
    Words(usize),
    Chips(Vec<usize>),
    Voice(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Piece {
    pub kind: PieceKind,
    /// From the top of the item's contents, and how tall.
    pub y: f32,
    pub h: f32,
}

/// Lay `parts` out, top down: the pieces and how tall they are together.
/// (`lines`: the words of each text part, wrapped, by the part's id.)
pub(crate) fn pieces(
    parts: &[Item],
    lines: &HashMap<u64, Vec<String>>,
    zoom: f32,
    open: bool,
) -> (Vec<Piece>, f32) {
    let text_line = (TEXT_LINE * zoom).round();
    let mut out: Vec<Piece> = Vec::new();
    let mut y = 0.0f32;
    for (n, part) in parts.iter().enumerate() {
        let file = matches!(part.kind, Kind::Image | Kind::File | Kind::Folder);
        if let (
            true,
            Some(Piece {
                kind: PieceKind::Chips(row),
                ..
            }),
        ) = (file, out.last_mut())
        {
            row.push(n);
            continue;
        }
        if !out.is_empty() {
            y += PIECE_GAP;
        }
        let (kind, h) = match part.kind {
            Kind::Text => (
                PieceKind::Words(n),
                lines.get(&part.id).map_or(1, Vec::len).max(1) as f32 * text_line,
            ),
            // (Open: what is said in it stands under its row.)
            Kind::Voice => (
                PieceKind::Voice(n),
                VOICE_ROW
                    + match lines.get(&part.id).map_or(0, Vec::len) {
                        said if open && said > 0 => 5.0 + said as f32 * text_line,
                        _ => 0.0,
                    },
            ),
            _ => (PieceKind::Chips(vec![n]), THUMB + 3.0 + THUMB_NAME),
        };
        out.push(Piece { kind, y, h });
        y += h;
    }
    (out, y)
}

/// How tall an item made of `parts` is.
pub(crate) fn mixed_height(
    parts: &[Item],
    lines: &HashMap<u64, Vec<String>>,
    zoom: f32,
    open: bool,
) -> f32 {
    2.0 * TILE_PAD_Y - 3.0 + NOTE_LINE + pieces(parts, lines, zoom, open).1
}

/// Where each part of an item is on screen (its id, its box, whether it is
/// a voice note), the item's contents starting at `at`, `w` wide: what the
/// pointer is tested against once the item is OPEN and its pieces answer
/// by themselves.
pub(crate) fn piece_rects(
    parts: &[Item],
    lines: &HashMap<u64, Vec<String>>,
    zoom: f32,
    open: bool,
    at: (f32, f32),
    w: f32,
) -> Vec<(u64, Rect, bool)> {
    let mut out = Vec::new();
    for piece in pieces(parts, lines, zoom, open).0 {
        let y = at.1 + piece.y;
        match piece.kind {
            PieceKind::Words(n) => out.push((parts[n].id, Rect::new(at.0, y, w, piece.h), false)),
            PieceKind::Voice(n) => out.push((parts[n].id, Rect::new(at.0, y, w, piece.h), true)),
            PieceKind::Chips(row) => {
                for (i, n) in row.iter().enumerate() {
                    out.push((
                        parts[*n].id,
                        Rect::new(at.0 + i as f32 * (THUMB + THUMB_GAP), y, THUMB, piece.h),
                        false,
                    ));
                }
            }
        }
    }
    out
}

/// Draw the pieces of `parts` into `grid`, their contents' top-left corner
/// at `at`, `w` wide.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_pieces(
    grid: &mut GridContent,
    parts: &[Item],
    lines: &HashMap<u64, Vec<String>>,
    slots: &HashMap<String, u32>,
    at: (f32, f32),
    w: f32,
    zoom: f32,
    paint: &Paint,
    clip: Rect,
    // The item is OPEN (its voice notes show what they say); the part the
    // pointer is on (it wears an item's rim then: it answers by itself);
    // the voice note being played, and for how many seconds now.
    open: bool,
    hot: Option<u64>,
    playing: Option<(u64, f32)>,
) {
    let (text_px, text_line) = (TEXT_PX * zoom, (TEXT_LINE * zoom).round());
    let bright = paint.bright();
    // (A piece under the pointer: the rim an item has, a hair out from it.)
    if let Some((_, r, _)) = hot.and_then(|id| {
        piece_rects(parts, lines, zoom, open, at, w)
            .into_iter()
            .find(|(pid, ..)| *pid == id)
    }) {
        let around = Rect::new(r.x - 4.0, r.y - 3.0, r.w + 8.0, r.h + 6.0);
        grid.rects.push(RectInst {
            rect: around,
            radius: 7.0,
            color: paint.ink_at(0.05),
            glass: 0.0,
            border: 0.0,
        });
        grid.rects.push(RectInst {
            rect: around,
            radius: 7.0,
            color: paint.ink_at(0.24),
            glass: 0.0,
            border: 1.0,
        });
    }
    let label = |grid: &mut GridContent,
                 text: &str,
                 pos: (f32, f32),
                 max_w: f32,
                 px: f32,
                 line: f32,
                 centered: bool,
                 family: Option<&'static str>,
                 color: [f32; 4]| {
        grid.labels.push(Label {
            text: text.to_owned(),
            pos,
            max_w,
            font_px: px,
            line_px: line,
            centered,
            dim: false,
            cache: true,
            clip: Some(clip),
            family,
            color: Some(color),
        });
    };
    for piece in pieces(parts, lines, zoom, open).0 {
        let y = at.1 + piece.y;
        match piece.kind {
            PieceKind::Words(n) => {
                let part = &parts[n];
                for (i, line) in lines.get(&part.id).into_iter().flatten().enumerate() {
                    if !line.is_empty() {
                        label(
                            grid,
                            line,
                            (at.0, y + i as f32 * text_line),
                            w,
                            text_px,
                            text_line,
                            false,
                            Some(crate::options::NERD),
                            paint.ink_at(0.92),
                        );
                    }
                }
            }
            PieceKind::Chips(row) => {
                for (i, n) in row.iter().enumerate() {
                    let part = &parts[*n];
                    let face = Rect::new(at.0 + i as f32 * (THUMB + THUMB_GAP), y, THUMB, THUMB);
                    if face.x + face.w > at.0 + w + 1.0 {
                        break;
                    }
                    let layer = part.path.as_ref().and_then(|p| slots.get(p));
                    let picture = part.kind == Kind::Image && layer.is_some();
                    grid.rects.push(RectInst {
                        rect: face,
                        radius: 7.0,
                        color: if picture {
                            [0.0, 0.0, 0.0, if bright { 0.08 } else { 0.30 }]
                        } else {
                            // (A page: the paper a document is.)
                            [0.925, 0.914, 0.886, 1.0]
                        },
                        glass: 0.0,
                        border: 0.0,
                    });
                    if let (true, Some(&layer)) = (picture, layer) {
                        grid.icons.push(IconInst {
                            rect: face,
                            layer,
                            tint: [0.0; 4],
                            ring: -1.0,
                            plate: NO_PLATE,
                        });
                    } else {
                        // Its lines of writing, and its kind in a corner.
                        for (l, part_w) in [1.0f32, 0.8, 1.0, 0.55].into_iter().enumerate() {
                            grid.rects.push(RectInst {
                                rect: Rect::new(
                                    face.x + 8.0,
                                    face.y + 10.0 + l as f32 * 8.0,
                                    (face.w - 16.0) * part_w,
                                    3.0,
                                ),
                                radius: 1.0,
                                color: [0.0, 0.0, 0.0, 0.22],
                                glass: 0.0,
                                border: 0.0,
                            });
                        }
                        let ext: String = part
                            .path
                            .as_deref()
                            .and_then(|p| std::path::Path::new(p).extension())
                            .map(|e| e.to_string_lossy().to_uppercase())
                            .unwrap_or_else(|| {
                                if part.kind == Kind::Folder {
                                    "DIR".to_owned()
                                } else {
                                    "FILE".to_owned()
                                }
                            })
                            .chars()
                            .take(4)
                            .collect();
                        let badge =
                            Rect::new(face.x + face.w - 30.0, face.y + face.h - 17.0, 26.0, 13.0);
                        grid.rects.push(RectInst {
                            rect: badge,
                            radius: 3.0,
                            color: [0.753, 0.314, 0.302, 1.0],
                            glass: 0.0,
                            border: 0.0,
                        });
                        label(
                            grid,
                            &ext,
                            (badge.x + badge.w / 2.0, badge.y + 1.0),
                            badge.w,
                            8.0,
                            11.0,
                            true,
                            None,
                            [1.0, 1.0, 1.0, 1.0],
                        );
                    }
                    // Its name, under it.
                    let name = part
                        .path
                        .as_deref()
                        .and_then(|p| std::path::Path::new(p).file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| part.body.clone());
                    let short: String = if name.chars().count() > 11 {
                        format!("{}…", name.chars().take(10).collect::<String>())
                    } else {
                        name
                    };
                    label(
                        grid,
                        &short,
                        (face.x + face.w / 2.0, face.y + face.h + 3.0),
                        THUMB + 6.0,
                        9.5,
                        THUMB_NAME - 1.0,
                        true,
                        None,
                        paint.ink_at(0.7),
                    );
                }
            }
            PieceKind::Voice(n) => {
                let part = &parts[n];
                let row = Rect::new(at.0, y, w, VOICE_ROW);
                let now = playing
                    .filter(|(id, _)| *id == part.id)
                    .map(|(_, secs)| secs);
                grid.rects.push(RectInst {
                    rect: row,
                    radius: 7.0,
                    color: paint.ink_at(0.06),
                    glass: 0.0,
                    border: 0.0,
                });
                label(
                    grid,
                    if now.is_some() {
                        "\u{f04d}"
                    } else {
                        "\u{f04b}"
                    },
                    (row.x + 16.0, row.y + 8.0),
                    20.0,
                    10.0,
                    14.0,
                    true,
                    Some(crate::options::NERD),
                    paint.ink_at(0.9),
                );
                let track = Rect::new(
                    row.x + 32.0,
                    row.y + row.h / 2.0 - 1.0,
                    (row.w - 84.0).max(0.0),
                    2.0,
                );
                grid.rects.push(RectInst {
                    rect: track,
                    radius: 1.0,
                    color: paint.ink_at(0.2),
                    glass: 0.0,
                    border: 0.0,
                });
                let secs = part.aspect.max(1.0);
                if let Some(heard) = now {
                    let acc = paint.accent();
                    grid.rects.push(RectInst {
                        rect: Rect::new(
                            track.x,
                            track.y,
                            track.w * (heard / secs).clamp(0.0, 1.0),
                            track.h,
                        ),
                        radius: 1.0,
                        color: [acc[0], acc[1], acc[2], 1.0],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                let total = secs.round() as u32;
                label(
                    grid,
                    &format!("{:02}:{:02}", total / 60, total % 60),
                    (row.x + row.w - 26.0, row.y + 8.0),
                    44.0,
                    10.5,
                    14.0,
                    true,
                    None,
                    paint.ink_at(0.65),
                );
                // What is said in it, under its row, while the item is open.
                if open {
                    for (i, line) in lines.get(&part.id).into_iter().flatten().enumerate() {
                        label(
                            grid,
                            line,
                            (at.0 + 3.0, row.y + VOICE_ROW + 5.0 + i as f32 * text_line),
                            w - 6.0,
                            text_px,
                            text_line,
                            false,
                            None,
                            paint.ink_at(0.9),
                        );
                    }
                }
            }
        }
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

    /// The card's colour as hue, saturation and lightness (in sRGB, where
    /// a step of lightness looks like one).
    fn hsl(&self) -> (f32, f32, f32) {
        use crate::options::{linear_to_srgb, rgb_to_hsl};
        rgb_to_hsl(
            linear_to_srgb(self.fill[0]).clamp(0.0, 1.0),
            linear_to_srgb(self.fill[1]).clamp(0.0, 1.0),
            linear_to_srgb(self.fill[2]).clamp(0.0, 1.0),
        )
    }

    /// THE ACCENT is the card's own colour, stronger and brighter (Max,
    /// 2026-10-10 — it was a fixed orange): the same hue, pushed up in
    /// saturation and lightness, so a blue-grey card has a bright blue one
    /// and a green card a bright green one. A card with no hue of its own
    /// (a grey) gets none invented for it. On a light card it is the deep
    /// tone instead of the bright one, to stand out as well.
    pub(super) fn accent(&self) -> [f32; 3] {
        use crate::options::hsl_to_rgb;
        let (h, s, _) = self.hsl();
        let hued = ((s - 0.02) / 0.06).clamp(0.0, 1.0);
        let (sat, light) = if self.bright() {
            (0.70, 0.36)
        } else {
            (0.78, 0.76)
        };
        // (As the ink is given: these are the values the words are drawn
        // with — linearised, the bright blue came out a dark one.)
        let (r, g, b) = hsl_to_rgb(h, sat * hued, light);
        [r, g, b]
    }

    /// The card's colour a step of lightness away from itself (lighter on
    /// a dark card, darker on a light one), solid.
    fn tone(&self, step: f32) -> [f32; 4] {
        use crate::options::{hsl_to_rgb, srgb_to_linear};
        let (h, s, l) = self.hsl();
        let l = if self.bright() { l - step } else { l + step };
        let (r, g, b) = hsl_to_rgb(h, s, l.clamp(0.0, 1.0));
        [
            srgb_to_linear(r),
            srgb_to_linear(g),
            srgb_to_linear(b),
            ITEM,
        ]
    }

    /// THE ZEBRA (Max, 2026-10-10: *"we need three colors, the card color,
    /// and then zebra 1 and 2"*): what sits on the card is drawn in two
    /// tones of the card's own colour. The LIGHTER (`true`) is the buttons'
    /// and every second item's; the other is the button's of the page that
    /// is up and the first item's — the alternation runs down from the
    /// buttons. (The steps were settled by eye on ~/terminal-mockup/card.)
    pub(super) fn zebra(&self, lighter: bool) -> [f32; 4] {
        self.tone(if lighter { ZEBRA_2 } else { ZEBRA_1 })
    }
}

/// The two zebra tones, as steps of lightness from the card's colour.
const ZEBRA_1: f32 = 0.025;
const ZEBRA_2: f32 = 0.07;
/// A button under the pointer: a step past the lighter tone.
const ZEBRA_HOT: f32 = 0.11;

/// How solid an item's box (and the input box, and the picker) is.
pub(super) const ITEM: f32 = 0.96;

/// The colour of a box that sits on the card: the card's own, a shade
/// darker, solid.
pub(super) fn solid(paint: &Paint, bright: bool) -> [f32; 4] {
    let shade = if bright { 0.93 } else { 0.72 };
    [
        paint.fill[0] * shade,
        paint.fill[1] * shade,
        paint.fill[2] * shade,
        ITEM,
    ]
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
    /// How big the items are drawn (1 = as designed): their text, their
    /// lines, their pictures. Ctrl +/− over the card (`App::card_zoom`).
    pub zoom: f32,
}

/// THE ZOOM is of the whole card (Max, 2026-10-09: *"i want ctrl + +- to
/// affect all the card, including buttons, icons, emojis, input, all"*):
/// everything is laid out as if the card were `1/zoom` its size, about its
/// own top-left corner (`virt`), and the finished scene is scaled up from
/// that corner (`zoom_scene`). The pointer is brought into the same space
/// before it is tested (`Card::inside`), and the tiles are kept in it.
pub(crate) fn virt(rect: Rect, zoom: f32) -> Rect {
    Rect::new(rect.x, rect.y, rect.w / zoom, rect.h / zoom)
}

/// Scale a scene laid out in `virt` space up by `zoom` about `origin`.
/// The card's own frame — its corners, its rim, its shadow — stays as it
/// is at any zoom: it is the card that holds, its contents that grow.
pub(crate) fn zoom_scene(scene: &mut Scene, origin: (f32, f32), zoom: f32) {
    if (zoom - 1.0).abs() < 0.001 {
        return;
    }
    let r = |rect: &mut Rect| {
        rect.x = origin.0 + (rect.x - origin.0) * zoom;
        rect.y = origin.1 + (rect.y - origin.1) * zoom;
        rect.w *= zoom;
        rect.h *= zoom;
    };
    for shadow in &mut scene.shadows {
        r(&mut shadow.rect);
    }
    for (n, grid) in scene.grids.iter_mut().enumerate() {
        r(&mut grid.clip);
        for rect in &mut grid.rects {
            r(&mut rect.rect);
            // (Grid 0 is the card's own frame.)
            if n > 0 {
                rect.radius *= zoom;
                rect.border *= zoom;
            }
        }
        for icon in &mut grid.icons {
            r(&mut icon.rect);
        }
        for label in &mut grid.labels {
            label.pos.0 = origin.0 + (label.pos.0 - origin.0) * zoom;
            label.pos.1 = origin.1 + (label.pos.1 - origin.1) * zoom;
            label.max_w *= zoom;
            label.font_px *= zoom;
            label.line_px *= zoom;
            if let Some(clip) = label.clip.as_mut() {
                r(clip);
            }
        }
    }
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
    let acc = paint.accent();
    let zoom = view.zoom;
    // The items' own sizes, at the card's zoom.
    let (text_px, text_line) = (TEXT_PX * zoom, (TEXT_LINE * zoom).round());
    let (kind_px, kind_line) = (KIND_PX * zoom, (KIND_LINE * zoom).round());
    let pic_h = (PIC_H * zoom).round();

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
        // The card's own ground lets what is behind it through (Max,
        // 2026-10-09: *"make the 'air' of the card more transparent"*);
        // the compositor frosts it (`declare_layer_rule`). The items on it
        // keep their own darker boxes, so they stay as readable as before.
        color: [
            paint.fill[0],
            paint.fill[1],
            paint.fill[2],
            paint.fill[3] * AIR,
        ],
        glass: 0.0,
        border: 0.0,
    });
    let rim = if view.dnd_over {
        [acc[0], acc[1], acc[2], 0.7]
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
    let low = view.foot.as_ref().map(|f| {
        super::bar::bottom(
            rect,
            f.page,
            !f.bar.query.is_empty() || f.bar.field == Some(Field::Seek),
            f.bar.picking,
        )
    });
    let body = list_rect(
        rect,
        view.foot.as_ref().map(|f| f.page),
        low.map_or(0.0, |b| b.h),
    );
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
    // Something being written is in the list (Max, 2026-10-10: *"enter
    // creates a new item and i can type on it"*): a new thing has its box
    // after the last item; an item being changed has it in its own place.
    let writing = view.foot.as_ref().and_then(|f| f.writing);
    // (The box: the parts already put in, then the line being written.)
    let sealed: &[Item] = view.foot.as_ref().map_or(&[], |f| f.sealed);
    let draft_h = view
        .foot
        .as_ref()
        .map_or(0.0, |f| super::bar::box_h(f.bar.box_lines()));
    let sealed_h = if sealed.is_empty() {
        0.0
    } else {
        TILE_PAD_Y + pieces(sealed, view.lines, zoom, false).1
    };
    let box_h = sealed_h + draft_h;
    let heights: Vec<f32> = shown_items
        .iter()
        .map(|it| match page {
            _ if writing == Some(Some(it.id)) => box_h,
            Page::Memory => ROW_H,
            _ if !it.parts.is_empty() => mixed_height(
                &it.parts,
                view.lines,
                zoom,
                view.foot.as_ref().is_some_and(|f| f.wide.contains(&it.id)),
            ),
            _ => tile_height(it.kind, view.lines.get(&it.id).map_or(1, Vec::len), zoom),
        })
        .collect();
    // (The opening for something dragged in is part of the list's length.)
    let opening = shown_items
        .iter()
        .filter_map(|it| view.shifts.get(&it.id))
        .fold(0.0f32, |a, s| a.max(*s));
    // (An open memory's name floats over the top of its list: the items
    // start under it, and slide behind it when the list is scrolled.)
    let named = view
        .foot
        .as_ref()
        .is_some_and(|f| f.page == Page::Session && f.bar.name.is_some());
    let over = if named { NAME_SPACE } else { 0.0 };
    let total = 2.0 * LIST_PAD
        + over
        + heights.iter().sum::<f32>()
        + GAP * heights.len().saturating_sub(1) as f32
        + opening
        + if writing == Some(None) {
            box_h + if heights.is_empty() { 0.0 } else { GAP }
        } else {
            0.0
        };
    let max_scroll = (total - body.h).max(0.0);
    let scroll = view.scroll.clamp(0.0, max_scroll);

    // An item's box is SOLID: the card's own colour, a shade down — the
    // colour it had when it was a dark wash over an opaque card. (The
    // card's ground is see-through now, and a wash over that let the
    // window behind show through the items too — Max, 2026-10-09: *"the
    // card looks good, but not the items. those need to be less
    // transparent"*.)
    //
    // In ZEBRA tones (Max, 2026-10-10): the first item the darker of the
    // two, the second the lighter, and so on down what is on screen.
    let tile_fills = [paint.zebra(false), paint.zebra(true)];
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
    let mut y = body.y + LIST_PAD + over - scroll;
    for (nth, (item, h)) in shown_items.iter().copied().zip(heights).enumerate() {
        let shift = view.shifts.get(&item.id).copied().unwrap_or(0.0);
        let tile = Tile {
            id: item.id,
            rect: Rect::new(rect.x + LIST_PAD, (y + shift).round(), tile_w, h),
            shift,
        };
        y += h + GAP;
        // (An item being changed: its place is the writing box's.)
        if writing == Some(Some(item.id)) {
            tiles.push(Tile { id: WRITER, ..tile });
            continue;
        }
        tiles.push(tile);
        let t = tile.rect;
        if t.y > clip.y + clip.h || t.y + t.h < clip.y {
            continue;
        }
        list.rects.push(RectInst {
            rect: t,
            radius: TILE_RADIUS,
            color: tile_fills[nth % 2],
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
                [acc[0], acc[1], acc[2], 0.55]
            } else {
                tile_rim
            },
            glass: 0.0,
            border: 1.0,
        });
        let note = view.foot.as_ref().and_then(|f| f.notes.get(&item.id));
        if page == Page::Memory {
            // A SESSION's row: its name only, and at the right when its
            // last thing was (Max, 2026-10-10: *"get rid of the avatar on
            // the memories, and the content, i want only the title"*, then
            // the date and time at the right, to tell two of one name apart).
            let row = view.lines.get(&item.id).map(Vec::as_slice).unwrap_or(&[]);
            let name = row.first().map(String::as_str).unwrap_or("");
            let mut put =
                |text: &str, pos: (f32, f32), w: f32, px: f32, centered: bool, color: [f32; 4]| {
                    list.labels.push(Label {
                        text: text.to_owned(),
                        pos,
                        max_w: w,
                        font_px: px,
                        line_px: (px * 1.35).round(),
                        centered,
                        dim: false,
                        cache: true,
                        clip: Some(clip),
                        family: None,
                        color: Some(color),
                    });
                };
            let tx = t.x + 13.0;
            let when = note.map(String::as_str).unwrap_or("");
            let when_w = super::bar::about(when.chars().count(), NOTE_PX + 1.0) + 8.0;
            // (The room at the right end is the ×'s, and a pencil's to come.)
            let tw = t.x + t.w - tx - when_w - 56.0;
            let mid = t.y + (t.h - 17.0) / 2.0;
            // (Its × clicked once: it asks for the second click, in red.
            // Being renamed: what is typed, with the cursor.)
            let foot = view.foot.as_ref();
            let arming = foot.is_some_and(|f| f.arming == Some(item.id));
            let renaming = foot
                .and_then(|f| f.renaming)
                .filter(|(id, _)| *id == item.id)
                .map(|(_, typed)| format!("{typed}|"));
            let danger = [DANGER[0], DANGER[1], DANGER[2], 1.0];
            match (&renaming, arming) {
                (Some(typed), _) => put(
                    typed,
                    (tx, mid),
                    tw,
                    12.5,
                    false,
                    [acc[0], acc[1], acc[2], 1.0],
                ),
                (_, true) => put(
                    "Forget this session? Click × again",
                    (tx, mid),
                    tw + when_w,
                    12.5,
                    false,
                    danger,
                ),
                _ => put(name, (tx, mid), tw, 12.5, false, paint.ink_at(0.96)),
            }
            if !arming {
                put(
                    when,
                    (t.x + t.w - when_w / 2.0 - 52.0, mid + 2.0),
                    when_w,
                    NOTE_PX + 1.0,
                    true,
                    paint.ink_at(0.5),
                );
            }
            let choosing = view.foot.as_ref().is_some_and(|f| f.choosing);
            let over = view.hover.is_some_and(|h| h.item() == Some(item.id));
            if !choosing && (over || arming) {
                let close = tile.close();
                let hot = view.hover == Some(Hover::Close(item.id));
                put(
                    "×",
                    (close.x + close.w / 2.0, close.y + 1.0),
                    close.w,
                    15.0,
                    true,
                    if hot || arming {
                        danger
                    } else {
                        paint.ink_at(0.45)
                    },
                );
            }
            // The pencil, left of the ×: the session is renamed in its row.
            if !choosing && over && !arming && renaming.is_none() {
                let pen = tile.pin();
                let hot = view.hover == Some(Hover::Edit(item.id));
                list.labels.push(Label {
                    text: GLYPH_EDIT.to_owned(),
                    pos: (pen.x + pen.w / 2.0, pen.y + 3.0),
                    max_w: pen.w,
                    font_px: 12.0,
                    line_px: CLOSE - 2.0,
                    centered: true,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family: Some(crate::options::NERD),
                    color: Some(if hot {
                        [acc[0], acc[1], acc[2], 1.0]
                    } else {
                        paint.ink_at(0.45)
                    }),
                });
            }
            continue;
        }
        let x = t.x + TILE_PAD_X;
        let mut line_y = t.y + TILE_PAD_Y;
        // (Made of several things: its pieces, in their order. It has no
        // lines of its own, so the rest of this finds nothing to draw but
        // its foot and its buttons.)
        if !item.parts.is_empty() {
            draw_pieces(
                &mut list,
                &item.parts,
                view.lines,
                view.slots,
                (x, line_y),
                text_w,
                zoom,
                paint,
                clip,
                view.foot
                    .as_ref()
                    .is_some_and(|f| f.wide.contains(&item.id)),
                // (Its pieces answer by themselves only once it is open.)
                view.hover
                    .and_then(|h| match h {
                        Hover::Item(id) | Hover::Play(id) => Some(id),
                        _ => None,
                    })
                    .filter(|id| item.parts.iter().any(|p| p.id == *id)),
                view.foot.as_ref().and_then(|f| f.playing),
            );
        }
        if let Some(word) = item.kind.word() {
            list.labels.push(Label {
                text: word.to_owned(),
                pos: (x, line_y),
                max_w: text_w,
                font_px: kind_px,
                line_px: kind_line,
                centered: false,
                dim: false,
                cache: true,
                clip: Some(clip),
                family: None,
                color: Some([acc[0], acc[1], acc[2], 0.85]),
            });
            line_y += kind_line;
        }
        if item.kind == Kind::Voice {
            // A VOICE NOTE (Max, 2026-10-10, settled on the mockup): the
            // play symbol alone — no colour, no box — with the note's title
            // beside it; under them a line that fills as it plays, between
            // where it is and how long it is; and "Transcribe" at its foot,
            // which shows what is said in it.
            let play = tile.play();
            let now = view
                .foot
                .as_ref()
                .and_then(|f| f.playing)
                .filter(|(id, _)| *id == item.id)
                .map(|(_, secs)| secs);
            let mut put =
                |text: &str, pos: (f32, f32), w: f32, px: f32, centered: bool, family, color| {
                    list.labels.push(Label {
                        text: text.to_owned(),
                        pos,
                        max_w: w,
                        font_px: px,
                        line_px: 14.0,
                        centered,
                        dim: false,
                        cache: true,
                        clip: Some(clip),
                        family,
                        color: Some(color),
                    });
                };
            put(
                if now.is_some() {
                    "\u{f04d}"
                } else {
                    "\u{f04b}"
                },
                (x + 14.0, t.y + TILE_PAD_Y + 2.0),
                play.w,
                11.0,
                true,
                Some(crate::options::NERD),
                paint.ink_at(0.92),
            );
            // Its title (one of its own, or what it is: dimmer).
            let named = !item.body.starts_with(VOICE_NAME);
            put(
                if named {
                    item.body.as_str()
                } else {
                    VOICE_NAME
                },
                (x + 32.0, t.y + TILE_PAD_Y + 1.0),
                text_w - 32.0 - 3.0 * CLOSE,
                TEXT_PX + 0.5,
                false,
                None,
                paint.ink_at(if named { 0.95 } else { 0.5 }),
            );
            let secs = item.aspect.max(1.0);
            let heard = now.unwrap_or(0.0).min(secs);
            let clock = |s: f32| {
                let s = s.round() as u32;
                format!("{:02}:{:02}", s / 60, s % 60)
            };
            let row = t.y + TILE_PAD_Y + 22.0;
            put(
                &clock(heard),
                (x + 14.0, row),
                34.0,
                10.0,
                true,
                None,
                paint.ink_at(0.6),
            );
            put(
                &clock(secs),
                (x + text_w - 14.0, row),
                34.0,
                10.0,
                true,
                None,
                paint.ink_at(0.6),
            );
            let track = Rect::new(x + 36.0, row + 6.0, (text_w - 72.0).max(0.0), 2.0);
            list.rects.push(RectInst {
                rect: track,
                radius: 1.0,
                color: paint.ink_at(0.18),
                glass: 0.0,
                border: 0.0,
            });
            let at = track.w * (heard / secs).clamp(0.0, 1.0);
            list.rects.push(RectInst {
                rect: Rect::new(track.x, track.y, at, track.h),
                radius: 1.0,
                color: [acc[0], acc[1], acc[2], 1.0],
                glass: 0.0,
                border: 0.0,
            });
            list.rects.push(RectInst {
                rect: Rect::new((track.x + at - 1.0).round(), track.y - 4.0, 2.0, 10.0),
                radius: 1.0,
                color: if now.is_some() {
                    [acc[0], acc[1], acc[2], 1.0]
                } else {
                    paint.ink_at(0.9)
                },
                glass: 0.0,
                border: 0.0,
            });
            // "Transcribe", in the middle of its foot (the word alone).
            let wide = view
                .foot
                .as_ref()
                .is_some_and(|f| f.wide.contains(&item.id));
            let more = tile.more();
            let hot = view.hover == Some(Hover::More(item.id));
            let busy = item.said.as_deref() == Some(WRITING_OUT);
            put(
                match (busy, wide) {
                    (true, _) => "Transcribing…",
                    (_, true) => "Hide text",
                    _ => "Transcribe",
                },
                (more.x + more.w / 2.0, more.y + 3.0),
                more.w,
                NOTE_PX + 1.0,
                true,
                None,
                if hot && !busy {
                    [acc[0], acc[1], acc[2], 1.0]
                } else {
                    paint.ink_at(if busy { 0.4 } else { 0.6 })
                },
            );
            // (What is said in it comes under these rows, as lines.)
            line_y = t.y + TILE_PAD_Y + VOICE_HEAD + 4.0;
        }
        if item.kind == Kind::Image {
            let frame = Rect::new(x, line_y, text_w, pic_h);
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
                    (pic_h * item.aspect).min(frame.w)
                } else {
                    pic_h
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
            line_y += pic_h + PIC_GAP;
        }
        let family = (item.kind == Kind::Text).then_some(crate::options::NERD);
        for (n, line) in view.lines.get(&item.id).into_iter().flatten().enumerate() {
            if !line.is_empty() {
                list.labels.push(Label {
                    text: line.clone(),
                    pos: (x, line_y),
                    max_w: text_w,
                    font_px: text_px,
                    line_px: text_line,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family,
                    // (A session in memory: its date stands out, what is
                    // in it sits back.)
                    color: Some(paint.ink_at(if n > 9000 { 0.0 } else { 0.92 })),
                });
            }
            line_y += text_line;
        }
        // A LONG text shows its first lines and "Show all" (Max,
        // 2026-10-10): a click on that opens all of it, and folds it again.
        // (A click on the item still pastes all of it; a drag carries all.)
        if item.kind == Kind::Text && item.parts.is_empty() {
            let wide = view
                .foot
                .as_ref()
                .is_some_and(|f| f.wide.contains(&item.id));
            let all = view.lines.get(&item.id).map_or(&[][..], Vec::as_slice);
            if has_more(all, wide) {
                let r = tile.more();
                let hot = view.hover == Some(Hover::More(item.id));
                list.labels.push(Label {
                    text: if wide { "Show less" } else { "Show all" }.to_owned(),
                    pos: (r.x + r.w / 2.0, r.y + 3.0),
                    max_w: r.w,
                    font_px: NOTE_PX + 1.0,
                    line_px: NOTE_LINE,
                    centered: true,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family: None,
                    color: Some(if hot {
                        [acc[0], acc[1], acc[2], 1.0]
                    } else {
                        paint.ink_at(0.6)
                    }),
                });
            }
        }
        // The line at its foot: when it is from (to the right).
        if let Some(note) = note {
            let w = super::bar::about(note.chars().count(), NOTE_PX) + 6.0;
            list.labels.push(Label {
                text: note.clone(),
                pos: (
                    t.x + t.w - TILE_PAD_X - w / 2.0,
                    t.y + t.h - NOTE_LINE - 5.0,
                ),
                max_w: w + 20.0,
                font_px: NOTE_PX,
                line_px: NOTE_LINE,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(clip),
                family: None,
                color: Some(paint.ink_at(0.45)),
            });
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
        // The pencil, on what can be written again: a text of one's own.
        if over.is_some()
            && matches!(item.kind, Kind::Text | Kind::Voice)
            && matches!(page, Page::Session | Page::Pinned)
        {
            button(
                tile.edit(true),
                GLYPH_EDIT,
                Some(crate::options::NERD),
                12.0,
                over == Some(Hover::Edit(item.id)),
                over == Some(Hover::Edit(item.id)),
                acc,
            );
        }
        if over.is_some() && page != Page::Dictionary {
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
        // THE DISK, on every item of every list (Max, 2026-10-10: *"add the
        // floppy everywhere, everything can be saved to memory… get rid of
        // the pin button"*): its menu keeps the item somewhere — Pin ·
        // Memory · New session.
        if over.is_some() {
            let hot = over == Some(Hover::Pin(item.id));
            button(
                tile.pin(),
                GLYPH_DISK,
                Some(crate::options::NERD),
                12.0,
                hot,
                hot,
                acc,
            );
        }
    }
    // AN EMPTY LIST: a dashed frame filling it, saying what a drop there
    // does (Max, 2026-10-10: *"one of those '----' frames with the hint,
    // 'drag here to'… on every empty tab"*, then *"make the frame use all
    // the card, and the text a little bigger"*). It wears the accent while
    // something is held over the card.
    let words = match page {
        Page::Session => Some("Drag here to keep"),
        Page::Pinned => Some("Drag here to pin"),
        _ => None,
    };
    if let (true, None, Some(words)) = (shown_items.is_empty(), writing, words) {
        let f = Rect::new(
            rect.x + LIST_PAD,
            body.y + LIST_PAD + over,
            tile_w,
            (body.h - 2.0 * LIST_PAD - over).max(0.0),
        );
        let color = if view.dnd_over {
            [acc[0], acc[1], acc[2], 0.9]
        } else {
            paint.ink_at(0.2)
        };
        let (dash, gap, thick) = (7.0f32, 6.0f32, 1.5f32);
        let mut put = |r: Rect| {
            list.rects.push(RectInst {
                rect: r,
                radius: 0.0,
                color,
                glass: 0.0,
                border: 0.0,
            })
        };
        // (Its corners are left open, about the pills' rounding.)
        let inset = RADIUS;
        let mut x = f.x + inset;
        while x + dash <= f.x + f.w - inset {
            put(Rect::new(x, f.y, dash, thick));
            put(Rect::new(x, f.y + f.h - thick, dash, thick));
            x += dash + gap;
        }
        let mut y = f.y + inset;
        while y + dash <= f.y + f.h - inset {
            put(Rect::new(f.x, y, thick, dash));
            put(Rect::new(f.x + f.w - thick, y, thick, dash));
            y += dash + gap;
        }
        if f.h > 40.0 {
            list.labels.push(Label {
                text: words.to_owned(),
                pos: (f.x + f.w / 2.0, f.y + f.h / 2.0 - 10.0),
                max_w: f.w - 20.0,
                font_px: 14.0,
                line_px: 20.0,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(clip),
                family: None,
                color: Some(if view.dnd_over {
                    [acc[0], acc[1], acc[2], 1.0]
                } else {
                    paint.ink_at(0.5)
                }),
            });
        }
    }
    // A new thing being written: its box after the last item.
    if writing == Some(None) {
        tiles.push(Tile {
            id: WRITER,
            rect: Rect::new(rect.x + LIST_PAD, y.round(), tile_w, box_h),
            shift: 0.0,
        });
    }
    let writer = tiles.iter().find(|t| t.id == WRITER).map(|t| t.rect);
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
                rect.y + FOOT_H - 1.0,
                rect.w - 2.0 * LIST_PAD,
                1.0,
            ),
            radius: 0.0,
            color: line,
            glass: 0.0,
            border: 0.0,
        });
        // The page's name, small, centred under the buttons.
        if let Some(title) = foot.title.or(foot.page.title()) {
            bar.labels.push(Label {
                text: title.to_owned(),
                pos: (rect.x + rect.w / 2.0, rect.y + FOOT_H + 2.0),
                max_w: rect.w - 2.0 * LIST_PAD,
                font_px: NOTE_PX + 0.5,
                line_px: NOTE_LINE,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(window),
                family: None,
                color: Some(paint.ink_at(0.5)),
            });
        }
        for (which, r) in foot_buttons(rect) {
            let up = which.page() == foot.page;
            let hot = view.hover == Some(Hover::Foot(which));
            bar.rects.push(RectInst {
                rect: r,
                radius: if which.round() {
                    r.h / 2.0
                } else {
                    TILE_RADIUS
                },
                // (Zebra: a button is the lighter tone, the one of the page
                // that is up the other.)
                color: match (up, hot) {
                    (true, _) => paint.zebra(false),
                    (_, true) => paint.tone(ZEBRA_HOT),
                    _ => paint.zebra(true),
                },
                glass: 0.0,
                border: 0.0,
            });
            // (…with the rim the items have: Max, 2026-10-10.)
            bar.rects.push(RectInst {
                rect: r,
                radius: if which.round() {
                    r.h / 2.0
                } else {
                    TILE_RADIUS
                },
                color: tile_rim,
                glass: 0.0,
                border: 1.0,
            });
            // (The one that is up: an arrow pointing up — back to the session.)
            let icon = which.round() || up;
            bar.labels.push(Label {
                text: if up { GLYPH_UP } else { which.word() }.to_owned(),
                pos: (
                    r.x + r.w / 2.0,
                    r.y + (r.h - TEXT_LINE) / 2.0 + if icon { 1.0 } else { 0.0 },
                ),
                max_w: r.w,
                font_px: TEXT_PX,
                line_px: TEXT_LINE,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(window),
                family: icon.then_some(crate::options::NERD),
                color: Some(if up {
                    [acc[0], acc[1], acc[2], 1.0]
                } else {
                    paint.ink_at(if hot { 0.95 } else { 0.72 })
                }),
            });
        }
        // (The writing box: in the list's own clip, over the items.)
        let mut well = GridContent {
            clip,
            ..Default::default()
        };
        // (The writing is the lower part of the box; the parts over it.)
        let draft = writer.map(|w| Rect::new(w.x, w.y + w.h - draft_h, w.w, draft_h));
        super::bar::draw(
            &mut bar, &mut well, draft, writer, rect, body, window, foot.page, &foot.bar, paint,
            bright, view.hover,
        );
        if let (Some(w), false) = (writer, sealed.is_empty()) {
            draw_pieces(
                &mut well,
                sealed,
                view.lines,
                view.slots,
                (w.x + TILE_PAD_X, w.y + TILE_PAD_Y),
                tile_w - 2.0 * TILE_PAD_X - 34.0,
                zoom,
                paint,
                clip,
                false,
                None,
                None,
            );
        }
        scene.grids.push(bar);
        if writer.is_some() {
            scene.grids.push(well);
        }
        // The disk's menu: a small box of rows under the disk, over the list.
        if let Some(menu) = &foot.menu {
            // (Every label is drawn in one late pass, over every box: the
            // words under the menu are left out, not painted over.)
            // A line that starts left of the menu is cut where the menu
            // begins; one that starts under it is not drawn.
            let m = menu.rect;
            for grid in &mut scene.grids {
                let within = grid.clip;
                grid.labels.retain_mut(|l| {
                    let (x0, x1) = if l.centered {
                        (l.pos.0 - l.max_w / 2.0, l.pos.0 + l.max_w / 2.0)
                    } else {
                        (l.pos.0, l.pos.0 + l.max_w)
                    };
                    if l.pos.1 + l.line_px <= m.y
                        || l.pos.1 >= m.y + m.h
                        || x1 <= m.x
                        || x0 >= m.x + m.w
                    {
                        return true;
                    }
                    if x0 >= m.x || l.centered {
                        return false;
                    }
                    let c = l.clip.unwrap_or(within);
                    l.clip = Some(Rect::new(c.x, c.y, (m.x - c.x).max(0.0), c.h));
                    true
                });
            }
            let mut over = GridContent {
                clip: window,
                ..Default::default()
            };
            scene.shadows.push(ShadowInst {
                rect: menu.rect,
                radius: RADIUS,
                blur: 18.0,
                color: [0.0, 0.0, 0.0, 0.9],
                edges: [0.2, 0.4, 0.3, 0.3],
            });
            let mut fill = paint.tone(ZEBRA_HOT);
            fill[3] = 1.0;
            over.rects.push(RectInst {
                rect: menu.rect,
                radius: RADIUS,
                color: fill,
                glass: 0.0,
                border: 0.0,
            });
            over.rects.push(RectInst {
                rect: menu.rect,
                radius: RADIUS,
                color: paint.ink_at(0.14),
                glass: 0.0,
                border: 1.0,
            });
            for (n, row) in menu.rows.iter().enumerate() {
                let r = Rect::new(
                    menu.rect.x + MENU_PAD,
                    menu.rect.y + MENU_PAD + n as f32 * MENU_ROW,
                    menu.rect.w - 2.0 * MENU_PAD,
                    MENU_ROW,
                );
                let hot = menu.hot == Some(n);
                if hot {
                    over.rects.push(RectInst {
                        rect: r,
                        radius: 7.0,
                        color: [acc[0], acc[1], acc[2], 0.2],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                over.labels.push(Label {
                    text: (*row).to_owned(),
                    pos: (r.x + 9.0, r.y + (r.h - TEXT_LINE) / 2.0),
                    max_w: r.w - 12.0,
                    font_px: TEXT_PX + 0.5,
                    line_px: TEXT_LINE,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: Some(window),
                    family: None,
                    color: Some(if hot {
                        [acc[0], acc[1], acc[2], 1.0]
                    } else {
                        paint.ink_at(0.95)
                    }),
                });
            }
            scene.grids.push(over);
        }
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
    zoom: f32,
    lighter: bool,
    paint: &Paint,
    thumb: Option<&[u8]>,
    thumb_side: usize,
    text: &mut DrawText,
) -> Canvas {
    let bright = paint.bright();
    let acc = paint.accent();
    // The items' own sizes, at the card's zoom.
    let (text_px, text_line) = (TEXT_PX * zoom, (TEXT_LINE * zoom).round());
    let (kind_px, kind_line) = (KIND_PX * zoom, (KIND_LINE * zoom).round());
    let pic_h = (PIC_H * zoom).round();
    let height = tile_height(item.kind, lines.len(), zoom);
    let s = |v: f32| v * scale;
    // (Whole logical pixels: the buffer is shown at 1/`scale` of its size.)
    let mut canvas = Canvas::new(s(width.round()) as usize, s(height.round()) as usize);
    let whole = Rect::new(0.0, 0.0, canvas.w as f32, canvas.h as f32);
    // The card's own colour underneath, then the item's inset and rim —
    // the list's own values, so it looks in the hand as it did in the list
    // (Max, 2026-10-08: *"i dont want the item to change color when im
    // draging it"*).
    canvas.round_rect(whole, s(TILE_RADIUS), paint.fill, None);
    let rim = if bright {
        [0.0, 0.0, 0.0, 0.10]
    } else {
        [1.0, 1.0, 1.0, 0.06]
    };
    // (Its own zebra tone: `lighter` is the one it has in the list.)
    canvas.round_rect(whole, s(TILE_RADIUS), paint.zebra(lighter), None);
    canvas.round_rect(whole, s(TILE_RADIUS), rim, Some(s(1.0)));

    let x = TILE_PAD_X;
    let text_w = width - 2.0 * TILE_PAD_X;
    let mut y = TILE_PAD_Y;
    if let Some(word) = item.kind.word() {
        text(
            &mut canvas,
            word,
            s(kind_px),
            None,
            [acc[0], acc[1], acc[2], 0.85],
            (s(x), s(y)),
        );
        y += kind_line;
    }
    if item.kind == Kind::Image {
        let frame = Rect::new(s(x), s(y), s(text_w), s(pic_h));
        let fill = if bright {
            [0.0, 0.0, 0.0, 0.08]
        } else {
            [0.0, 0.0, 0.0, 0.30]
        };
        canvas.round_rect(frame, s(PIC_RADIUS), fill, None);
        if let Some(pixels) = thumb {
            let side = if item.aspect > 1.0 {
                (s(pic_h) * item.aspect).min(frame.w)
            } else {
                s(pic_h)
            };
            let to = Rect::new(
                frame.x + (frame.w - side) / 2.0,
                frame.y + (frame.h - side) / 2.0,
                side,
                side,
            );
            canvas.picture(pixels, thumb_side, to, frame);
        }
        y += pic_h + PIC_GAP;
    }
    let family = (item.kind == Kind::Text).then_some(crate::options::NERD);
    let ink = paint.ink_at(0.92);
    for line in lines {
        if !line.is_empty() {
            text(&mut canvas, line, s(text_px), family, ink, (s(x), s(y)));
        }
        y += text_line;
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
            parts: Vec::new(),
            said: None,
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
            parts: Vec::new(),
            said: None,
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
            zoom: 1.0,
        };
        let (scene, tiles, max_scroll) = scene(&view(0.0, 1.0));
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].rect.y, 60.0 + LIST_PAD);
        assert!(tiles[1].rect.y > tiles[0].rect.y && tiles[2].rect.y > tiles[1].rect.y);
        // Taller than the card: it scrolls, by exactly the overflow.
        let total = 2.0 * LIST_PAD
            + tile_height(Kind::Text, 1, 1.0)
            + tile_height(Kind::File, 1, 1.0)
            + tile_height(Kind::Image, 1, 1.0)
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
            zoom: 1.0,
        });
        assert_eq!(scene.grids[0].clip.h, 100.0);
        // The card under the clip is whole, and wears the drag's rim.
        assert_eq!(scene.grids[0].rects[0].rect.h, 400.0);
        assert_eq!(scene.grids[0].rects[1].color[..3], PAINT.accent());
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
                zoom: 1.0,
            })
            .1
        };
        let none = HashMap::new();
        let rest = view(&none, None);
        let h = tile_height(Kind::Text, 1, 1.0);
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
            1.0,
            false,
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
            (tile_height(Kind::Text, 1, 1.0) * 2.0).round() as usize
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
        let items: Vec<Item> = (1..=20).map(|n| text(n, "a")).collect();
        let lines: HashMap<u64, Vec<String>> = items
            .iter()
            .map(|it| (it.id, vec![it.body.clone()]))
            .collect();
        let (shifts, slots) = (HashMap::new(), HashMap::new());
        let notes: HashMap<u64, String> = HashMap::new();
        let wide: HashSet<u64> = HashSet::new();
        let rect = Rect::new(600.0, 60.0, WIDTH, 500.0);
        let view = |foot: bool| View {
            rect,
            shown: 1.0,
            items: &items,
            lines: &lines,
            scroll: 0.0,
            hover: None,
            foot: foot.then_some(FootView {
                page: Page::Session,
                current: None,
                bar: BarView {
                    name: Some("Golem new feature"),
                    naming: None,
                    draft: &[],
                    query: "",
                    found: 0,
                    field: None,
                    caret: None,
                    first: 0,
                    select: &[],
                    rec: None,
                    talk: None,
                    hint: "",
                    hint_lit: false,
                    hint_red: false,
                    picking: None,
                    emoji: &[],
                    pick_query: "",
                    kinds: &[],
                    recent: 0,
                },
                notes: &notes,
                playing: None,
                writing: None,
                title: None,
                choosing: false,
                menu: None,
                arming: None,
                renaming: None,
                sealed: &[],
                wide: &wide,
            }),
            dnd_over: false,
            hidden: None,
            shifts: &shifts,
            slots: &slots,
            paint: PAINT,
            zoom: 1.0,
        };
        let (bare, _, scroll_bare) = scene(&view(false));
        let (with, _, scroll_foot) = scene(&view(true));
        // The list is shorter by the head and by what is at the bottom, so
        // there is that much more to scroll.
        let low = crate::card::bar::bottom(rect, Page::Session, false, None);
        // (…and its first item starts under the memory's name.)
        assert_eq!(scroll_foot, scroll_bare + FOOT_H + low.h + NAME_SPACE);
        assert_eq!(with.grids.len(), bare.grids.len() + 1);
        // The buttons, in the head, side by side inside the card; the
        // list starts under them and ends over the input box.
        let buttons = foot_buttons(rect);
        assert_eq!(
            buttons.map(|(f, _)| f),
            [
                Foot::Emoji,
                Foot::Pinned,
                Foot::Clipboard,
                Foot::Dictionary,
                Foot::Memory
            ]
        );
        // The ones with an icon are round, from the left; Memory, by name,
        // has the rest of the row at the right.
        for (_, r) in &buttons[..4] {
            assert_eq!((r.w, r.h), (FOOT_BUTTON_H, FOOT_BUTTON_H));
        }
        assert!(buttons[4].1.w > 3.0 * FOOT_BUTTON_H);
        assert!(buttons[4].1.x + buttons[4].1.w <= rect.x + rect.w - LIST_PAD + 1.0);
        let list = list_rect(rect, Some(Page::Session), low.h);
        assert_eq!(list.y, rect.y + FOOT_H);
        // A page with a name has it under the buttons: its list starts lower.
        assert_eq!(
            list_rect(rect, Some(Page::Pinned), 0.0).y,
            rect.y + FOOT_H + TITLE_H
        );
        assert_eq!(Page::Memory.title(), Some("Sessions"));
        // (…and ends over the line of hint at the bottom.)
        assert_eq!(list.y + list.h, rect.y + rect.h - low.h);
        for (_, r) in buttons {
            assert!(r.y >= rect.y && r.y + r.h <= rect.y + FOOT_H);
            assert!(r.x >= rect.x && r.x + r.w <= rect.x + rect.w);
        }
        assert!(buttons[0].1.x + buttons[0].1.w <= buttons[1].1.x);
        // No item wears a mark of its own at rest: its buttons (the pencil,
        // the disk, the ×) show under the pointer only.
        let marks = |s: &Scene| {
            s.grids[1]
                .labels
                .iter()
                .filter(|l| l.text == GLYPH_DISK || l.text == GLYPH_EDIT)
                .count()
        };
        assert_eq!((marks(&bare), marks(&with)), (0, 0));
    }

    #[test]
    fn the_whole_card_zooms_about_its_corner_and_its_frame_holds() {
        let items = vec![text(1, "a")];
        let lines: HashMap<u64, Vec<String>> = HashMap::from([(1, vec!["a".to_owned()])]);
        let (shifts, slots) = (HashMap::new(), HashMap::new());
        let rect = Rect::new(600.0, 60.0, WIDTH, 400.0);
        let build = |rect: Rect| {
            scene(&View {
                rect,
                shown: 1.0,
                items: &items,
                lines: &lines,
                scroll: 0.0,
                hover: None,
                foot: None,
                dnd_over: false,
                hidden: None,
                shifts: &shifts,
                slots: &slots,
                paint: PAINT,
                zoom: 1.0,
            })
        };
        let (plain, tiles, _) = build(rect);
        // At twice the size the card is laid out half as big, and scaled up.
        let (mut big, small, _) = build(virt(rect, 2.0));
        zoom_scene(&mut big, (rect.x, rect.y), 2.0);
        // Its frame is the same box, the same corners.
        assert_eq!(big.grids[0].rects[0].rect, plain.grids[0].rects[0].rect);
        assert_eq!(big.grids[0].rects[0].radius, RADIUS);
        // Its first item starts twice as far from the corner, twice as tall,
        // its words twice as big.
        let (a, b) = (plain.grids[1].rects[0].rect, big.grids[1].rects[0].rect);
        assert_eq!(b.x - rect.x, 2.0 * (a.x - rect.x));
        assert_eq!(b.y - rect.y, 2.0 * (a.y - rect.y));
        assert_eq!(b.h, 2.0 * a.h);
        assert_eq!(
            big.grids[1].labels[0].font_px,
            2.0 * plain.grids[1].labels[0].font_px
        );
        // The tiles stay in the laid-out space (the pointer is brought to it).
        assert_eq!(small[0].rect.h, tiles[0].rect.h);
        // At 1 nothing moves.
        let (mut same, _, _) = build(rect);
        zoom_scene(&mut same, (rect.x, rect.y), 1.0);
        assert_eq!(same.grids[1].rects[0].rect, a);
    }
}

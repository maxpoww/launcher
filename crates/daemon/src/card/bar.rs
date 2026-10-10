//! The card's fixed furniture around the list (the mockup at
//! ~/terminal-mockup/chat, approved 2026-10-09): at the BOTTOM the search —
//! a circle with the magnifier that opens into a field — and under it the
//! input box a memory is written in (on Memory's page: New, in its place);
//! at the TOP of an open memory its name, floating over the items.

use super::view::{
    Hover, Page, Paint, ACCENT, DANGER, FOOT_H, GAP, LIST_PAD, RADIUS, TEXT_LINE, TEXT_PX,
};
use crate::content::{GridContent, Label, Rect, RectInst};
use crate::options::NERD;

/// What the typing goes into, while the card has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// The input box: a memory being written.
    Box,
    /// The search.
    Seek,
    /// The open memory's name.
    Name,
    /// The emoji picker's search.
    Pick,
}

/// The input box's own buttons: the paperclip on its left, the two of the
/// voice on its right. (It had an emoji button too; emoji have their own
/// page — the head's round button — and are for the WINDOW: Max, 2026-10-10.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxBtn {
    /// The paperclip: a file from this computer.
    Clip,
    /// Talk and it is written.
    Talk,
    /// Record a voice note.
    Mic,
}

impl BoxBtn {
    fn glyph(self) -> &'static str {
        match self {
            BoxBtn::Clip => "\u{f067}", // fa-plus
            // (Looked at, each one, in the font itself: a microphone with a
            // speech bubble for "talk and it is written" — the plain speech
            // bubble said nothing of talking; and the font's larger, solid
            // microphone for a voice note — the other was a size smaller
            // than its neighbours.)
            BoxBtn::Talk => "\u{f050a}", // md-microphone-message
            BoxBtn::Mic => "\u{f036c}",  // md-microphone
        }
    }

    /// How big its icon is drawn: each to look the same size as the rest
    /// (the icons are not all drawn to the same scale in the font).
    fn px(self) -> f32 {
        match self {
            BoxBtn::Talk => 25.0,
            BoxBtn::Mic => 22.0,
            BoxBtn::Clip => 20.0,
        }
    }

    pub(super) fn hint(self) -> &'static str {
        match self {
            BoxBtn::Clip => "Add a file from this computer",
            BoxBtn::Talk => "Talk and it is written here",
            BoxBtn::Mic => "Record a voice note",
        }
    }
}

const GLYPH_SEARCH: &str = "\u{f002}";

/// The emoji picker: EVERY emoji (the clipboard box's table,
/// `emoji_table::EMOJI`), on a grid that scrolls, under a field that
/// searches them by name. (GIFs and stickers, which the mockup shows beside
/// them, have no source yet.)
const PICK_ROWS: f32 = 6.0;
const PICK_SEEK: f32 = 30.0;
/// The row of kinds along its bottom (faces, people, animals, food…): a
/// click jumps the grid to that kind, as on a phone's keyboard.
const PICK_KINDS: f32 = 32.0;
const EMOJI_COLS: usize = 8;
const EMOJI_CELL: f32 = 34.0;

/// The search at rest (a circle) and open (a field).
const SEEK: f32 = 28.0;
const SEEK_OPEN_W: f32 = 220.0;
/// The input box: its least height, its buttons, the most lines it grows to.
const BOX_MIN: f32 = 48.0;
const BTN: f32 = 34.0;
/// How far one button is from the next: less than a button is wide —
/// their icons are smaller than they are, so they sit shoulder to shoulder.
const BTN_STEP: f32 = 22.0;
/// How far its buttons are from its edges, and its words from them.
const BTN_EDGE: f32 = -4.0;
const BTN_GAP: f32 = -5.0;
/// …and from the + on the left, which keeps the same room all round.
const PLUS_GAP: f32 = 6.0;
pub(super) const BOX_LINES: usize = 5;
const HINT_H: f32 = 16.0;
const PAD: f32 = 8.0;
const STEP: f32 = 6.0;
/// The name bubble.
const NAME_H: f32 = 26.0;

/// What the bar is drawn from.
pub(crate) struct BarView<'a> {
    /// The open memory's name (none: no bubble).
    pub name: Option<&'a str>,
    /// …and what is being typed over it, while it is renamed.
    pub naming: Option<&'a str>,
    /// What is written in the input box, wrapped to it.
    pub draft: &'a [String],
    pub query: &'a str,
    /// How many of the list match the search.
    pub found: usize,
    /// Which field has the cursor.
    pub field: Option<Field>,
    /// Where the writing cursor is in the input box: the line of `draft`
    /// it is on, and how far along it (px).
    pub caret: Option<(usize, f32)>,
    /// The first of its lines that is in view.
    pub first: usize,
    /// What is selected in it: for each line it touches, from where to
    /// where along that line (px).
    pub select: &'a [(usize, f32, f32)],
    /// A voice note is being recorded: for how many seconds now.
    pub rec: Option<u32>,
    /// Talking is being listened to / written out.
    pub talk: Option<&'a str>,
    /// The line under the input box.
    pub hint: &'a str,
    /// The emoji picker, when it is open: how many emoji it shows and
    /// how far its grid is scrolled; the emoji themselves (all, or those
    /// its search found); what is being searched.
    pub picking: Option<(usize, f32)>,
    pub emoji: &'a [&'static str],
    pub pick_query: &'a str,
    /// One emoji standing for each kind (the row along its bottom), and
    /// how many of the first emoji shown are the RECENT ones.
    pub kinds: &'a [&'static str],
    pub recent: usize,
}

impl BarView<'_> {
    fn seeking(&self) -> bool {
        self.field == Some(Field::Seek) || !self.query.is_empty()
    }
}

/// Where everything at the bottom is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bottom {
    /// How much of the card's height it takes.
    pub h: f32,
    pub seek: Rect,
    /// The × of an open search.
    pub seek_clear: Rect,
    /// The input box (an open memory only).
    pub input: Option<Rect>,
    pub btns: [(BoxBtn, Rect); 3],
    /// New (Memory's page only).
    pub new: Option<Rect>,
    /// The emoji picker, open over the input box.
    pub tray: Option<Rect>,
    /// How many emoji it holds, and how far its grid is scrolled.
    pick: (usize, f32),
    /// How many rows of them are in view.
    rows: f32,
    hint_y: f32,
}

/// Lay the bottom out for `page`: from the card's lower edge up — the hint
/// line and the input box (or New), then the search over them.
pub(crate) fn bottom(
    rect: Rect,
    page: Page,
    box_lines: usize,
    seeking: bool,
    picking: Option<(usize, f32)>,
) -> Bottom {
    let (x, w) = (rect.x + LIST_PAD - 2.0, rect.w - 2.0 * LIST_PAD + 4.0);
    let mut y = rect.y + rect.h - PAD;
    let mut input = None;
    let mut new = None;
    let mut tray = None;
    let mut hint_y = y;
    match page {
        Page::Session => {
            y -= HINT_H;
            hint_y = y;
            let lines = box_lines.clamp(1, BOX_LINES) as f32;
            let h = (lines * TEXT_LINE + 2.0 * 14.0).max(BOX_MIN).round();
            y -= h;
            input = Some(Rect::new(x, y.round(), w, h));
            y -= STEP;
            if picking.is_some() {
                let h = PICK_SEEK + PICK_ROWS * EMOJI_CELL + PICK_KINDS + 14.0;
                y -= h;
                tray = Some(Rect::new(x, y.round(), w, h));
                y -= STEP;
            }
        }
        Page::Memory => {
            y -= BOX_MIN;
            new = Some(Rect::new(x, y.round(), w, BOX_MIN));
            y -= STEP;
        }
        Page::Pinned | Page::Clipboard | Page::Dictionary => {}
        // The emoji's own page: the picker is all of it, under the head
        // (it has its own search; the card's is not shown).
        Page::Emoji => {
            let top = rect.y + FOOT_H + STEP;
            let tray = Rect::new(x, top.round(), w, (y - top).max(0.0).round());
            let rows = ((tray.h - PICK_SEEK - PICK_KINDS - 14.0) / EMOJI_CELL)
                .floor()
                .max(1.0);
            return Bottom {
                h: rect.h - FOOT_H,
                seek: Rect::new(rect.x, rect.y, 0.0, 0.0),
                seek_clear: Rect::new(rect.x, rect.y, 0.0, 0.0),
                input: None,
                btns: [BoxBtn::Clip, BoxBtn::Talk, BoxBtn::Mic]
                    .map(|b| (b, Rect::new(rect.x, rect.y, 0.0, 0.0))),
                new: None,
                tray: Some(tray),
                pick: picking.unwrap_or((0, 0.0)),
                rows,
                hint_y: y,
            };
        }
    }
    y -= SEEK;
    let seek_w = if seeking { SEEK_OPEN_W.min(w) } else { SEEK };
    let seek = Rect::new(
        (rect.x + (rect.w - seek_w) / 2.0).round(),
        y.round(),
        seek_w,
        SEEK,
    );
    let seek_clear = Rect::new(seek.x + seek.w - 24.0, seek.y + 4.0, 20.0, 20.0);
    y -= STEP;
    let btn_y = input.map_or(0.0, |r| r.y + r.h - BTN - (BOX_MIN - BTN) / 2.0);
    // (The + has the same room on every side — to the box's left edge, to
    // its top and bottom, to the words: Max, 2026-10-10. Its mark is about
    // 13 px in a box 48 tall, so that room is about 17.)
    let left = x + BTN_EDGE + 11.0;
    let right = x + w - BTN_EDGE - BTN;
    let btns = [
        (BoxBtn::Clip, Rect::new(left, btn_y, BTN, BTN)),
        (BoxBtn::Talk, Rect::new(right - BTN_STEP, btn_y, BTN, BTN)),
        (BoxBtn::Mic, Rect::new(right, btn_y, BTN, BTN)),
    ];
    Bottom {
        h: rect.y + rect.h - y,
        seek,
        seek_clear,
        input,
        btns,
        new,
        tray,
        pick: picking.unwrap_or((0, 0.0)),
        rows: PICK_ROWS,
        hint_y,
    }
}

impl Bottom {
    /// Which of the input box's lines (of `lines`, the first in view being
    /// `first`) is at height `y`, and how far along it `x` is.
    pub(crate) fn text_at(
        &self,
        pos: (f32, f32),
        lines: usize,
        first: usize,
    ) -> Option<(usize, f32)> {
        let (r, text) = (self.input?, self.text()?);
        let shown = lines.clamp(1, BOX_LINES);
        let top = r.y + (r.h - shown as f32 * TEXT_LINE) / 2.0;
        let row = ((pos.1 - top) / TEXT_LINE)
            .floor()
            .clamp(0.0, shown as f32 - 1.0) as usize;
        Some((
            (first + row).min(lines.saturating_sub(1)),
            (pos.0 - text.x).max(0.0),
        ))
    }

    /// The part of the input box the words are in.
    pub(crate) fn text(&self) -> Option<Rect> {
        let r = self.input?;
        let left = self.btns[0].1.x + BTN + PLUS_GAP;
        let right = self.btns[1].1.x - BTN_GAP;
        Some(Rect::new(left, r.y, (right - left).max(0.0), r.h))
    }

    /// The picker's search row, and the grid under it.
    pub(crate) fn pick_seek(&self) -> Option<Rect> {
        let tray = self.tray?;
        Some(Rect::new(
            tray.x + 6.0,
            tray.y + 6.0,
            tray.w - 12.0,
            PICK_SEEK - 6.0,
        ))
    }

    pub(crate) fn pick_grid(&self) -> Option<Rect> {
        let tray = self.tray?;
        Some(Rect::new(
            tray.x + 6.0,
            tray.y + PICK_SEEK + 4.0,
            tray.w - 12.0,
            self.rows * EMOJI_CELL,
        ))
    }

    /// The kinds along the picker's bottom, each with its cell.
    pub(crate) fn kinds(&self) -> impl Iterator<Item = (usize, Rect)> + '_ {
        let tray = self.tray;
        let count = crate::emoji_table::GROUPS.len();
        (0..count).filter_map(move |kind| {
            let tray = tray?;
            let cell = (tray.w - 12.0) / count as f32;
            Some((
                kind,
                Rect::new(
                    tray.x + 6.0 + kind as f32 * cell,
                    tray.y + tray.h - PICK_KINDS - 4.0,
                    cell,
                    PICK_KINDS,
                ),
            ))
        })
    }

    /// How far down the grid the emoji at place `n` is: the scroll that
    /// brings its row to the top.
    pub(crate) fn pick_row(n: usize) -> f32 {
        (n / EMOJI_COLS) as f32 * EMOJI_CELL
    }

    /// How far the picker's grid is scrolled.
    pub(crate) fn pick_scroll(&self) -> f32 {
        self.pick.1
    }

    /// How far the picker's grid can be scrolled.
    pub(crate) fn pick_span(&self) -> f32 {
        let rows = self.pick.0.div_ceil(EMOJI_COLS) as f32;
        ((rows - self.rows) * EMOJI_CELL).max(0.0)
    }

    /// The picker's emoji that are in view, each with its cell (by its
    /// place among those shown).
    pub(crate) fn emoji(&self) -> impl Iterator<Item = (usize, Rect)> + '_ {
        let grid = self.pick_grid();
        let (count, scroll) = self.pick;
        let first = (scroll / EMOJI_CELL).floor().max(0.0) as usize * EMOJI_COLS;
        let last = (first + (self.rows as usize + 1) * EMOJI_COLS).min(count);
        (first..last).filter_map(move |n| {
            let grid = grid?;
            let cell = grid.w / EMOJI_COLS as f32;
            Some((
                n,
                Rect::new(
                    grid.x + (n % EMOJI_COLS) as f32 * cell,
                    grid.y + (n / EMOJI_COLS) as f32 * EMOJI_CELL - scroll,
                    cell,
                    EMOJI_CELL,
                ),
            ))
        })
    }

    /// What of the bottom is under `pos`.
    pub(crate) fn hit(&self, pos: (f32, f32), seeking: bool) -> Option<Hover> {
        if seeking && self.seek_clear.contains(pos) {
            return Some(Hover::SeekClear);
        }
        if self.seek.contains(pos) {
            return Some(Hover::Seek);
        }
        if self.new.is_some_and(|r| r.contains(pos)) {
            return Some(Hover::New);
        }
        if self.pick_seek().is_some_and(|r| r.contains(pos)) {
            return Some(Hover::PickSeek);
        }
        if self.pick_grid().is_some_and(|r| r.contains(pos)) {
            return Some(
                self.emoji()
                    .find(|(_, r)| r.contains(pos))
                    .map_or(Hover::Tray, |(n, _)| Hover::Emoji(n)),
            );
        }
        if let Some((kind, _)) = self.kinds().find(|(_, r)| r.contains(pos)) {
            return Some(Hover::Kind(kind));
        }
        if self.tray.is_some_and(|r| r.contains(pos)) {
            return Some(Hover::Tray);
        }
        self.input.filter(|r| r.contains(pos))?;
        Some(
            self.btns
                .iter()
                .find(|(_, r)| r.contains(pos))
                .map_or(Hover::Input, |(b, _)| Hover::Btn(*b)),
        )
    }
}

/// The first of the input box's lines that is in view: all of them while
/// they fit; past that, the ones up to the line the cursor is on.
pub(crate) fn first_line(lines: usize, caret: Option<usize>) -> usize {
    let over = lines.saturating_sub(BOX_LINES);
    match caret {
        Some(line) => (line + 1).saturating_sub(BOX_LINES).min(over),
        None => over,
    }
}

/// How wide a line of `chars` characters is, roughly (the bubble and the
/// right-hand notes are sized without a font to ask).
pub(super) fn about(chars: usize, px: f32) -> f32 {
    chars as f32 * px * 0.56
}

/// Where the open memory's name floats: centred, at the top of the list.
pub(crate) fn name_rect(list: Rect, name: &str) -> Rect {
    let w = (about(name.chars().count().max(4), 12.5) + 30.0).min(list.w - 2.0 * LIST_PAD);
    Rect::new(
        (list.x + (list.w - w) / 2.0).round(),
        list.y + 7.0,
        w.round(),
        NAME_H,
    )
}

fn label(
    text: &str,
    pos: (f32, f32),
    max_w: f32,
    px: f32,
    centered: bool,
    color: [f32; 4],
    clip: Rect,
) -> Label {
    Label {
        text: text.to_owned(),
        pos,
        max_w,
        font_px: px,
        line_px: (px * 1.35).round(),
        centered,
        dim: false,
        cache: true,
        clip: Some(clip),
        family: None,
        color: Some(color),
    }
}

fn glyph(g: &str, r: Rect, px: f32, color: [f32; 4], clip: Rect) -> Label {
    Label {
        text: g.to_owned(),
        pos: (r.x + r.w / 2.0, r.y + (r.h - px * 1.35) / 2.0),
        max_w: r.w,
        font_px: px,
        line_px: (px * 1.35).round(),
        centered: true,
        dim: false,
        cache: true,
        clip: Some(clip),
        family: Some(NERD),
        color: Some(color),
    }
}

fn boxed(
    grid: &mut GridContent,
    rect: Rect,
    radius: f32,
    fill: [f32; 4],
    rim: [f32; 4],
    width: f32,
) {
    grid.rects.push(RectInst {
        rect,
        radius,
        color: fill,
        glass: 0.0,
        border: 0.0,
    });
    grid.rects.push(RectInst {
        rect,
        radius,
        color: rim,
        glass: 0.0,
        border: width,
    });
}

/// Draw the bottom, and the name over the list.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    grid: &mut GridContent,
    rect: Rect,
    list: Rect,
    clip: Rect,
    page: Page,
    bar: &BarView,
    paint: &Paint,
    bright: bool,
    hover: Option<Hover>,
) {
    let ink = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a];
    let accent = |a: f32| [ACCENT[0], ACCENT[1], ACCENT[2], a];
    // (Solid, as the items' boxes are: the card's ground is see-through.)
    let well = super::view::solid(paint, bright);
    let rim = if bright {
        [0.0, 0.0, 0.0, 0.12]
    } else {
        [1.0, 1.0, 1.0, 0.07]
    };
    // (What floats over the items is solid enough to be read over them.)
    let plate = [
        paint.fill[0] * 0.72,
        paint.fill[1] * 0.72,
        paint.fill[2] * 0.72,
        0.94,
    ];
    let b = bottom(rect, page, bar.draft.len(), bar.seeking(), bar.picking);

    // The name of the memory, floating at the top.
    if let (Page::Session, Some(name)) = (page, bar.name) {
        let naming = bar.field == Some(Field::Name);
        let shown = if naming {
            bar.naming.unwrap_or("")
        } else {
            name
        };
        let text = if naming {
            format!("{shown}|")
        } else {
            shown.to_owned()
        };
        let r = name_rect(list, &text);
        let hot = hover == Some(Hover::Name);
        boxed(
            grid,
            r,
            RADIUS,
            plate,
            if naming {
                accent(1.0)
            } else if hot {
                accent(0.5)
            } else {
                rim
            },
            if naming { 1.5 } else { 1.0 },
        );
        grid.labels.push(label(
            &text,
            (r.x + r.w / 2.0, r.y + 4.0),
            r.w - 12.0,
            12.5,
            true,
            if naming { ink(0.95) } else { accent(1.0) },
            clip,
        ));
    }

    // The search: a circle with the magnifier; open, a field. (Not on the
    // emoji's page, whose picker has its own.)
    if b.seek.w > 0.0 {
        let seeking = bar.seeking();
        let on_seek = matches!(hover, Some(Hover::Seek | Hover::SeekClear));
        boxed(
            grid,
            b.seek,
            if seeking { RADIUS } else { SEEK / 2.0 },
            plate,
            if seeking {
                accent(1.0)
            } else if on_seek {
                accent(0.5)
            } else {
                rim
            },
            if seeking { 1.5 } else { 1.0 },
        );
        let lens = Rect::new(b.seek.x, b.seek.y, SEEK, SEEK);
        grid.labels.push(glyph(
            GLYPH_SEARCH,
            lens,
            11.0,
            if seeking { accent(1.0) } else { ink(0.6) },
            clip,
        ));
        if seeking {
            let caret = if bar.field == Some(Field::Seek) {
                "|"
            } else {
                ""
            };
            grid.labels.push(label(
                &format!("{}{caret}", bar.query),
                (b.seek.x + SEEK, b.seek.y + 6.0),
                b.seek.w - SEEK - 84.0,
                12.0,
                false,
                ink(0.95),
                clip,
            ));
            // (How many were found — where the search narrows a list.)
            if !bar.query.is_empty() && bar.found != usize::MAX {
                let found = format!("{} found", bar.found);
                grid.labels.push(label(
                    &found,
                    (
                        b.seek_clear.x - about(found.chars().count(), 10.0) / 2.0 - 6.0,
                        b.seek.y + 8.0,
                    ),
                    80.0,
                    10.0,
                    true,
                    ink(0.5),
                    clip,
                ));
            }
            grid.labels.push(label(
                "×",
                (b.seek_clear.x + b.seek_clear.w / 2.0, b.seek_clear.y),
                b.seek_clear.w,
                14.0,
                true,
                ink(if hover == Some(Hover::SeekClear) {
                    0.95
                } else {
                    0.5
                }),
                clip,
            ));
        }
    }

    // New, in the input box's place.
    if let Some(r) = b.new {
        let hot = hover == Some(Hover::New);
        boxed(
            grid,
            r,
            RADIUS,
            accent(if hot { 0.07 } else { 0.025 }),
            accent(0.45),
            1.0,
        );
        grid.labels.push(label(
            "+   New",
            (r.x + r.w / 2.0, r.y + (r.h - 18.0) / 2.0),
            r.w,
            13.0,
            true,
            accent(1.0),
            clip,
        ));
    }

    // The emoji picker, over the input box.
    if let (Some(tray), Some(seek), Some(cells)) = (b.tray, b.pick_seek(), b.pick_grid()) {
        boxed(grid, tray, RADIUS, well, rim, 1.0);
        // Its search.
        let searching = bar.field == Some(Field::Pick);
        boxed(
            grid,
            seek,
            7.0,
            ink(0.04),
            if searching { accent(0.7) } else { rim },
            1.0,
        );
        grid.labels.push(glyph(
            GLYPH_SEARCH,
            Rect::new(seek.x, seek.y, seek.h, seek.h),
            10.0,
            ink(0.5),
            clip,
        ));
        let (words, color) = match (bar.pick_query.is_empty(), searching) {
            (true, false) => ("Search emoji".to_owned(), ink(0.42)),
            (_, true) => (format!("{}|", bar.pick_query), ink(0.95)),
            (false, false) => (bar.pick_query.to_owned(), ink(0.95)),
        };
        grid.labels.push(label(
            &words,
            (seek.x + seek.h + 2.0, seek.y + 4.0),
            seek.w - seek.h - 8.0,
            11.5,
            false,
            color,
            clip,
        ));
        // The grid, cut to its own box (it scrolls).
        let window = Rect::new(
            cells.x,
            cells.y.max(clip.y),
            cells.w,
            (cells.h).min((clip.y + clip.h - cells.y).max(0.0)),
        );
        if bar.emoji.is_empty() {
            grid.labels.push(label(
                "No emoji by that name",
                (cells.x + cells.w / 2.0, cells.y + 30.0),
                cells.w,
                11.5,
                true,
                ink(0.5),
                clip,
            ));
        }
        // The kinds, along its bottom.
        for (kind, cell) in b.kinds() {
            let Some(face) = bar.kinds.get(kind) else {
                continue;
            };
            if hover == Some(Hover::Kind(kind)) {
                grid.rects.push(RectInst {
                    rect: Rect::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0),
                    radius: 7.0,
                    color: ink(0.10),
                    glass: 0.0,
                    border: 0.0,
                });
            }
            grid.labels.push(Label {
                text: (*face).to_owned(),
                pos: (cell.x + cell.w / 2.0, cell.y + 7.0),
                max_w: cell.w,
                font_px: 13.0,
                line_px: 18.0,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(clip),
                family: Some(crate::options::EMOJI_FONT),
                color: Some([
                    1.0,
                    1.0,
                    1.0,
                    if hover == Some(Hover::Kind(kind)) {
                        1.0
                    } else {
                        0.7
                    },
                ]),
            });
        }
        // The ones used lately come first, on a faint band of their own.
        if bar.recent > 0 && bar.pick_query.is_empty() {
            let rows = bar.recent.div_ceil(EMOJI_COLS) as f32;
            let top = cells.y - b.pick_scroll();
            let band = Rect::new(
                cells.x,
                top.max(cells.y),
                cells.w,
                (top + rows * EMOJI_CELL - top.max(cells.y)).max(0.0),
            );
            if band.h > 0.0 {
                grid.rects.push(RectInst {
                    rect: band,
                    radius: 7.0,
                    color: accent(0.06),
                    glass: 0.0,
                    border: 0.0,
                });
            }
        }
        for (n, cell) in b.emoji() {
            let Some(emoji) = bar.emoji.get(n) else {
                continue;
            };
            // (The hover wash is not cut to the grid's box as the labels
            // are: only a row wholly in it wears one.)
            let whole = cell.y >= cells.y - 1.0 && cell.y + cell.h <= cells.y + cells.h + 1.0;
            if whole && hover == Some(Hover::Emoji(n)) {
                grid.rects.push(RectInst {
                    rect: Rect::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0),
                    radius: 7.0,
                    color: ink(0.10),
                    glass: 0.0,
                    border: 0.0,
                });
            }
            grid.labels.push(Label {
                text: (*emoji).to_owned(),
                pos: (cell.x + cell.w / 2.0, cell.y + 6.0),
                max_w: cell.w,
                font_px: 17.0,
                line_px: 22.0,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(window),
                family: Some(crate::options::EMOJI_FONT),
                color: Some([1.0, 1.0, 1.0, 1.0]),
            });
        }
    }

    // The input box.
    if let (Some(r), Some(text)) = (b.input, b.text()) {
        let typing = bar.field == Some(Field::Box);
        boxed(
            grid,
            r,
            RADIUS,
            well,
            if typing { accent(0.6) } else { rim },
            1.0,
        );
        for (which, br) in b.btns {
            let hot = hover == Some(Hover::Btn(which));
            let live = match which {
                BoxBtn::Mic => bar.rec.is_some(),
                BoxBtn::Talk => bar.talk.is_some(),
                BoxBtn::Clip => false,
            };
            if bar.rec.is_some() && which != BoxBtn::Mic {
                continue;
            }
            if live || hot {
                grid.rects.push(RectInst {
                    rect: Rect::new(br.x + 6.0, br.y + 4.0, br.w - 12.0, br.h - 8.0),
                    radius: 7.0,
                    color: match (live, which) {
                        (true, BoxBtn::Mic) => [DANGER[0], DANGER[1], DANGER[2], 0.9],
                        (true, _) => accent(0.9),
                        _ => ink(0.08),
                    },
                    glass: 0.0,
                    border: 0.0,
                });
            }
            let color = if live {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                ink(if hot { 0.95 } else { 0.55 })
            };
            grid.labels
                .push(glyph(which.glyph(), br, which.px(), color, clip));
        }
        let top = r.y + (r.h - bar.draft.len().clamp(1, BOX_LINES) as f32 * TEXT_LINE) / 2.0;
        if let Some(secs) = bar.rec {
            grid.labels.push(label(
                &format!("●  {}:{:02}   recording", secs / 60, secs % 60),
                (r.x + 16.0, top),
                r.w - BTN - 30.0,
                TEXT_PX + 0.5,
                false,
                [DANGER[0], DANGER[1], DANGER[2], 1.0],
                clip,
            ));
        } else {
            let empty = bar.draft.is_empty() || (bar.draft.len() == 1 && bar.draft[0].is_empty());
            let first = bar.first.min(bar.draft.len().saturating_sub(1));
            // What is selected, behind the words.
            for (line, from, to) in bar.select {
                if *line < first || *line >= first + BOX_LINES {
                    continue;
                }
                grid.rects.push(RectInst {
                    rect: Rect::new(
                        (text.x + from).round(),
                        top + (line - first) as f32 * TEXT_LINE + 1.0,
                        (to - from).max(3.0).round(),
                        TEXT_LINE - 1.0,
                    ),
                    radius: 2.0,
                    color: accent(0.34),
                    glass: 0.0,
                    border: 0.0,
                });
            }
            if empty {
                // (Nothing written: what to do, unless the cursor is here.)
                let words = match bar.talk {
                    Some(say) => Some((say, accent(0.9))),
                    None if typing => None,
                    None => Some(("Write to this memory", ink(0.42))),
                };
                if let Some((words, color)) = words {
                    grid.labels.push(label(
                        words,
                        (text.x, top),
                        text.w,
                        TEXT_PX + 0.5,
                        false,
                        color,
                        clip,
                    ));
                }
            } else {
                for (n, line) in bar.draft.iter().skip(first).take(BOX_LINES).enumerate() {
                    grid.labels.push(label(
                        line,
                        (text.x, top + n as f32 * TEXT_LINE),
                        text.w + 40.0,
                        TEXT_PX + 0.5,
                        false,
                        ink(0.95),
                        clip,
                    ));
                }
            }
            // The writing cursor, where it is in the text.
            // (Not while it is scrolled out of view.)
            let seen = |line: usize| line >= first && line < first + BOX_LINES;
            if let (true, Some((line, x))) = (typing, bar.caret.filter(|c| seen(c.0))) {
                let row = (line - first) as f32;
                grid.rects.push(RectInst {
                    rect: Rect::new(
                        (text.x + x).round(),
                        top + row * TEXT_LINE + 2.0,
                        1.5,
                        TEXT_LINE - 3.0,
                    ),
                    radius: 0.0,
                    color: accent(1.0),
                    glass: 0.0,
                    border: 0.0,
                });
            }
        }
        if !bar.hint.is_empty() {
            grid.labels.push(label(
                bar.hint,
                (rect.x + rect.w / 2.0, b.hint_y + 2.0),
                rect.w - 2.0 * GAP,
                10.0,
                true,
                ink(0.5),
                clip,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bottom_is_laid_out_for_each_page() {
        let rect = Rect::new(600.0, 60.0, 420.0, 600.0);
        let low = rect.y + rect.h;
        // An open memory: the search over the input box, all inside the card.
        let b = bottom(rect, Page::Session, 1, false, None);
        let input = b.input.unwrap();
        assert!(b.new.is_none());
        assert!(b.seek.y + b.seek.h <= input.y && input.y + input.h < low);
        assert_eq!((b.seek.w, input.h), (SEEK, BOX_MIN));
        assert!((b.seek.x + b.seek.w / 2.0 - (rect.x + rect.w / 2.0)).abs() <= 1.0);
        // Its four buttons are inside it, two to each side of the words.
        let text = b.text().unwrap();
        for (n, (_, r)) in b.btns.iter().enumerate() {
            // (The icon is well inside; its hit box may run a hair past the edge.)
            let mid = r.x + r.w / 2.0;
            assert!(mid > input.x + 8.0 && mid < input.x + input.w - 8.0 && r.y >= input.y);
            // (Their icons — their middles — are to each side of the words.)
            assert_eq!(r.x + r.w / 2.0 < text.x, n < 1);
            assert_eq!(r.x + r.w / 2.0 > text.x + text.w, n >= 1);
        }
        // It grows with what is written, to a point; the search rides up with it.
        let tall = bottom(rect, Page::Session, 4, false, None);
        assert!(tall.input.unwrap().h > input.h && tall.seek.y < b.seek.y);
        assert_eq!(
            bottom(rect, Page::Session, 40, false, None),
            bottom(rect, Page::Session, BOX_LINES, false, None)
        );
        // Memory: New where the input box is. Pinned, Clipboard: the search alone, lower.
        let m = bottom(rect, Page::Memory, 1, false, None);
        assert_eq!(
            m.new.map(|r| (r.x, r.y, r.h)),
            Some((input.x, low - PAD - BOX_MIN, BOX_MIN))
        );
        assert!(m.input.is_none());
        let p = bottom(rect, Page::Pinned, 1, false, None);
        assert!(p.input.is_none() && p.new.is_none() && p.seek.y > b.seek.y && p.h < m.h);
        // The search opens sideways, about its middle.
        let open = bottom(rect, Page::Pinned, 1, true, None);
        assert_eq!(open.seek.w, SEEK_OPEN_W);
        assert!((open.seek.x + open.seek.w / 2.0 - (p.seek.x + p.seek.w / 2.0)).abs() <= 1.0);
    }

    #[test]
    fn the_pointer_finds_the_bottoms_parts() {
        let rect = Rect::new(0.0, 0.0, 420.0, 600.0);
        let b = bottom(rect, Page::Session, 1, false, None);
        let mid = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(b.hit(mid(b.seek), false), Some(Hover::Seek));
        assert_eq!(
            b.hit(mid(b.btns[2].1), false),
            Some(Hover::Btn(BoxBtn::Mic))
        );
        assert_eq!(b.hit(mid(b.text().unwrap()), false), Some(Hover::Input));
        assert_eq!(b.hit((5.0, 5.0), false), None);
        let m = bottom(rect, Page::Memory, 1, true, None);
        assert_eq!(m.hit(mid(m.new.unwrap()), true), Some(Hover::New));
        assert_eq!(m.hit(mid(m.seek_clear), true), Some(Hover::SeekClear));
        // The emoji picker opens over the input box, the search over it;
        // it shows the rows in view of however many emoji it holds.
        let p = bottom(rect, Page::Session, 1, false, Some((1800, 0.0)));
        let tray = p.tray.unwrap();
        assert!(tray.y + tray.h <= p.input.unwrap().y && p.seek.y + p.seek.h <= tray.y);
        assert_eq!(p.emoji().next().map(|(n, _)| n), Some(0));
        assert!(p.emoji().count() <= (PICK_ROWS as usize + 1) * EMOJI_COLS);
        let (n, cell) = p.emoji().nth(9).unwrap();
        assert_eq!(p.hit(mid(cell), false), Some(Hover::Emoji(n)));
        assert_eq!(
            p.hit(mid(p.pick_seek().unwrap()), false),
            Some(Hover::PickSeek)
        );
        // Scrolled two rows down, the same cell is a later emoji.
        let down = bottom(
            rect,
            Page::Session,
            1,
            false,
            Some((1800, 2.0 * EMOJI_CELL)),
        );
        assert_eq!(
            down.hit(mid(cell), false),
            Some(Hover::Emoji(n + 2 * EMOJI_COLS))
        );
        assert!(down.pick_span() > 0.0);
        // Its kinds run along its bottom, inside it, under the grid.
        let kinds: Vec<_> = p.kinds().collect();
        assert_eq!(kinds.len(), crate::emoji_table::GROUPS.len());
        let grid = p.pick_grid().unwrap();
        for (kind, r) in &kinds {
            assert!(r.y >= grid.y + grid.h && r.y + r.h <= tray.y + tray.h);
            assert_eq!(p.hit(mid(*r), false), Some(Hover::Kind(*kind)));
        }
        assert_eq!(Bottom::pick_row(2 * EMOJI_COLS + 3), 2.0 * EMOJI_CELL);
        // On the emoji's own page the picker is the whole card under the
        // head: more rows, no card search, nothing left for a list.
        let page = bottom(rect, Page::Emoji, 1, false, Some((1800, 0.0)));
        let all = page.tray.unwrap();
        assert!(all.y >= rect.y + FOOT_H && all.y + all.h <= rect.y + rect.h);
        assert!(page.emoji().count() > p.emoji().count());
        assert_eq!((page.seek.w, page.h), (0.0, rect.h - FOOT_H));
        assert!(page.kinds().all(|(_, r)| r.y + r.h <= all.y + all.h));
        assert!(b.tray.is_none() && b.emoji().next().is_none());
        // The input box shows the lines up to the one the cursor is on; a
        // point in it is a line and a distance along it.
        assert_eq!(first_line(3, Some(1)), 0);
        assert_eq!(first_line(9, None), 9 - BOX_LINES);
        assert_eq!(first_line(9, Some(0)), 0);
        assert_eq!(first_line(9, Some(6)), 7 - BOX_LINES);
        let text = b.text().unwrap();
        let at = b.text_at((text.x + 30.0, text.y + text.h / 2.0), 1, 0);
        assert_eq!(at, Some((0, 30.0)));
    }
}

//! The card's fixed furniture around the list (the mockup at
//! ~/terminal-mockup/chat, approved 2026-10-09): at the BOTTOM the search —
//! a circle with the magnifier that opens into a field — and under it the
//! input box a memory is written in (on Memory's page: New, in its place);
//! at the TOP of an open memory its name, floating over the items.

use super::view::{Hover, Page, Paint, ACCENT, DANGER, GAP, LIST_PAD, RADIUS, TEXT_LINE, TEXT_PX};
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
}

/// The input box's own buttons: two on its left, two on its right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxBtn {
    Emoji,
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
            BoxBtn::Emoji => "\u{f118}", // fa-smile-o
            BoxBtn::Clip => "\u{f0c6}",  // fa-paperclip
            BoxBtn::Talk => "\u{f27a}",  // fa-commenting
            BoxBtn::Mic => "\u{f130}",   // fa-microphone
        }
    }

    pub(super) fn hint(self) -> &'static str {
        match self {
            BoxBtn::Emoji => "Emoji",
            BoxBtn::Clip => "Add a file from this computer",
            BoxBtn::Talk => "Talk and it is written here",
            BoxBtn::Mic => "Record a voice note",
        }
    }
}

const GLYPH_SEARCH: &str = "\u{f002}";

/// The emoji the input box's picker offers: the ones most used, a few
/// signs. (The whole table is the clipboard box's picker's; GIFs and
/// stickers, which the mockup shows beside them, have no source yet.)
pub(crate) const EMOJI: [&str; 40] = [
    "👍", "😂", "🙏", "🔥", "❤️", "😅", "🎉", "👀", "😀", "😁", "🥹", "😊", "😉", "😍", "😎", "🤔",
    "😴", "😭", "😡", "🤯", "🥳", "🤝", "👏", "🙌", "💪", "👋", "✅", "❌", "⚠️", "💡", "📌", "🚀",
    "⭐", "💬", "📎", "🐛", "☕", "🍕", "🎧", "💤",
];
const EMOJI_COLS: usize = 8;
const EMOJI_CELL: f32 = 34.0;

/// The search at rest (a circle) and open (a field).
const SEEK: f32 = 28.0;
const SEEK_OPEN_W: f32 = 220.0;
/// The input box: its least height, its buttons, the most lines it grows to.
const BOX_MIN: f32 = 48.0;
const BTN: f32 = 36.0;
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
    /// A voice note is being recorded: for how many seconds now.
    pub rec: Option<u32>,
    /// Talking is being listened to / written out.
    pub talk: Option<&'a str>,
    /// The line under the input box.
    pub hint: &'a str,
    /// The emoji picker is open.
    pub picking: bool,
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
    pub btns: [(BoxBtn, Rect); 4],
    /// New (Memory's page only).
    pub new: Option<Rect>,
    /// The emoji picker, open over the input box.
    pub tray: Option<Rect>,
    hint_y: f32,
}

/// Lay the bottom out for `page`: from the card's lower edge up — the hint
/// line and the input box (or New), then the search over them.
pub(crate) fn bottom(
    rect: Rect,
    page: Page,
    box_lines: usize,
    seeking: bool,
    picking: bool,
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
            if picking {
                let rows = EMOJI.len().div_ceil(EMOJI_COLS) as f32;
                let h = rows * EMOJI_CELL + 12.0;
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
        Page::Pinned | Page::Clipboard => {}
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
    let btn_y = input.map_or(0.0, |r| r.y + r.h - BTN - 6.0);
    let left = x + 6.0;
    let right = x + w - 6.0 - BTN;
    let btns = [
        (BoxBtn::Emoji, Rect::new(left, btn_y, BTN, BTN)),
        (BoxBtn::Clip, Rect::new(left + BTN, btn_y, BTN, BTN)),
        (BoxBtn::Talk, Rect::new(right - BTN, btn_y, BTN, BTN)),
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
        hint_y,
    }
}

impl Bottom {
    /// The part of the input box the words are in.
    pub(crate) fn text(&self) -> Option<Rect> {
        let r = self.input?;
        let left = self.btns[1].1.x + BTN + 4.0;
        let right = self.btns[2].1.x - 4.0;
        Some(Rect::new(left, r.y, (right - left).max(0.0), r.h))
    }

    /// The picker's emoji, each with its cell.
    pub(crate) fn emoji(&self) -> impl Iterator<Item = (usize, Rect)> + '_ {
        let tray = self.tray;
        (0..EMOJI.len()).filter_map(move |n| {
            let tray = tray?;
            let cell = (tray.w - 12.0) / EMOJI_COLS as f32;
            Some((
                n,
                Rect::new(
                    tray.x + 6.0 + (n % EMOJI_COLS) as f32 * cell,
                    tray.y + 6.0 + (n / EMOJI_COLS) as f32 * EMOJI_CELL,
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
        if let Some((n, _)) = self.emoji().find(|(_, r)| r.contains(pos)) {
            return Some(Hover::Emoji(n));
        }
        let input = self.input.filter(|r| r.contains(pos))?;
        let _ = input;
        Some(
            self.btns
                .iter()
                .find(|(_, r)| r.contains(pos))
                .map_or(Hover::Input, |(b, _)| Hover::Btn(*b)),
        )
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
    let well = if bright {
        [0.0, 0.0, 0.0, 0.08]
    } else {
        [0.0, 0.0, 0.0, 0.30]
    };
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

    // The search: a circle with the magnifier; open, a field.
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
        if !bar.query.is_empty() {
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
    if let Some(tray) = b.tray {
        boxed(grid, tray, RADIUS, well, rim, 1.0);
        for (n, cell) in b.emoji() {
            if hover == Some(Hover::Emoji(n)) {
                grid.rects.push(RectInst {
                    rect: Rect::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0),
                    radius: 7.0,
                    color: ink(0.10),
                    glass: 0.0,
                    border: 0.0,
                });
            }
            grid.labels.push(Label {
                text: EMOJI[n].to_owned(),
                pos: (cell.x + cell.w / 2.0, cell.y + 6.0),
                max_w: cell.w,
                font_px: 17.0,
                line_px: 22.0,
                centered: true,
                dim: false,
                cache: true,
                clip: Some(clip),
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
                BoxBtn::Emoji => bar.picking,
                BoxBtn::Clip => false,
            };
            if bar.rec.is_some() && which != BoxBtn::Mic {
                continue;
            }
            if live || hot {
                grid.rects.push(RectInst {
                    rect: Rect::new(br.x + 2.0, br.y + 2.0, br.w - 4.0, br.h - 4.0),
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
                .push(glyph(which.glyph(), br, 15.0, color, clip));
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
        } else if bar.draft.is_empty() || (bar.draft.len() == 1 && bar.draft[0].is_empty()) {
            let (words, color) = match bar.talk {
                Some(say) => (say, accent(0.9)),
                None if typing => ("|", ink(0.95)),
                None => ("Write to this memory", ink(0.42)),
            };
            grid.labels.push(label(
                words,
                (text.x, top),
                text.w,
                TEXT_PX + 0.5,
                false,
                color,
                clip,
            ));
        } else {
            let first = bar.draft.len().saturating_sub(BOX_LINES);
            for (n, line) in bar.draft[first..].iter().enumerate() {
                let last = first + n + 1 == bar.draft.len();
                let shown = if last && typing {
                    format!("{line}|")
                } else {
                    line.clone()
                };
                grid.labels.push(label(
                    &shown,
                    (text.x, top + n as f32 * TEXT_LINE),
                    text.w,
                    TEXT_PX + 0.5,
                    false,
                    ink(0.95),
                    clip,
                ));
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
        let b = bottom(rect, Page::Session, 1, false, false);
        let input = b.input.unwrap();
        assert!(b.new.is_none());
        assert!(b.seek.y + b.seek.h <= input.y && input.y + input.h < low);
        assert_eq!((b.seek.w, input.h), (SEEK, BOX_MIN));
        assert!((b.seek.x + b.seek.w / 2.0 - (rect.x + rect.w / 2.0)).abs() <= 1.0);
        // Its four buttons are inside it, two to each side of the words.
        let text = b.text().unwrap();
        for (n, (_, r)) in b.btns.iter().enumerate() {
            assert!(r.x >= input.x && r.x + r.w <= input.x + input.w && r.y >= input.y);
            assert_eq!(r.x + r.w <= text.x, n < 2);
        }
        // It grows with what is written, to a point; the search rides up with it.
        let tall = bottom(rect, Page::Session, 4, false, false);
        assert!(tall.input.unwrap().h > input.h && tall.seek.y < b.seek.y);
        assert_eq!(
            bottom(rect, Page::Session, 40, false, false),
            bottom(rect, Page::Session, BOX_LINES, false, false)
        );
        // Memory: New where the input box is. Pinned, Clipboard: the search alone, lower.
        let m = bottom(rect, Page::Memory, 1, false, false);
        assert_eq!(
            m.new.map(|r| (r.x, r.y, r.h)),
            Some((input.x, low - PAD - BOX_MIN, BOX_MIN))
        );
        assert!(m.input.is_none());
        let p = bottom(rect, Page::Pinned, 1, false, false);
        assert!(p.input.is_none() && p.new.is_none() && p.seek.y > b.seek.y && p.h < m.h);
        // The search opens sideways, about its middle.
        let open = bottom(rect, Page::Pinned, 1, true, false);
        assert_eq!(open.seek.w, SEEK_OPEN_W);
        assert!((open.seek.x + open.seek.w / 2.0 - (p.seek.x + p.seek.w / 2.0)).abs() <= 1.0);
    }

    #[test]
    fn the_pointer_finds_the_bottoms_parts() {
        let rect = Rect::new(0.0, 0.0, 420.0, 600.0);
        let b = bottom(rect, Page::Session, 1, false, false);
        let mid = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(b.hit(mid(b.seek), false), Some(Hover::Seek));
        assert_eq!(
            b.hit(mid(b.btns[3].1), false),
            Some(Hover::Btn(BoxBtn::Mic))
        );
        assert_eq!(b.hit(mid(b.text().unwrap()), false), Some(Hover::Input));
        assert_eq!(b.hit((5.0, 5.0), false), None);
        let m = bottom(rect, Page::Memory, 1, true, false);
        assert_eq!(m.hit(mid(m.new.unwrap()), true), Some(Hover::New));
        assert_eq!(m.hit(mid(m.seek_clear), true), Some(Hover::SeekClear));
        // The emoji picker opens over the input box, the search over it.
        let p = bottom(rect, Page::Session, 1, false, true);
        let tray = p.tray.unwrap();
        assert!(tray.y + tray.h <= p.input.unwrap().y && p.seek.y + p.seek.h <= tray.y);
        assert_eq!(p.emoji().count(), EMOJI.len());
        let (n, cell) = p.emoji().nth(9).unwrap();
        assert_eq!(p.hit(mid(cell), false), Some(Hover::Emoji(n)));
        assert!(b.tray.is_none() && b.emoji().next().is_none());
    }
}

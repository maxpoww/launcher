//! The emoji picker — a mode of the clipboard box.
//!
//! The footer's 🙂 button wipes a scrollable grid of every emoji over the
//! history list; a click on one **types it into the window underneath** and the
//! box stays open so a message can take several. The box's own type-to-search
//! field filters the grid instead of the clips while it is up ("happy" → 😀,
//! via gemoji's keywords — see [`crate::emoji_table`]).
//!
//! **Colour** comes from asking for the font BY NAME
//! ([`crate::options::EMOJI_FONT`]): the sans-serif fallback chain resolves the
//! smiley block to DejaVu Sans, which carries monochrome outlines for it.
//!
//! **Typing** is the clipboard's own paste path — the emoji is handed to our
//! data-control source and a Ctrl+V is injected into the window below, which
//! includes handing the keyboard back for the moment it takes (Wayland offers
//! the clipboard only to the keyboard-focused client — see
//! [`App::paste_into_window_below`]). It is deliberately kept OUT of the
//! history: you asked to type an emoji, not to copy one. It does take over the
//! clipboard, exactly as any copy does.
//!
//! The target app must treat **Ctrl+V** as paste, which every GUI app does but
//! terminals do not (they want Ctrl+Shift+V) — the same limit the paste pill
//! has always had.

use crate::content::{Label, Rect, RectInst, Scene};
use crate::emoji_table::{EMOJI, GROUPS};
use crate::options::{hover_grow, EMOJI_FONT, FONT_PX, LINE_PX};
use crate::App;

/// Side inset of the grid inside the box.
const PAD: f32 = 10.0;
/// One (square) cell: the emoji plus the room its hover halo needs.
const CELL: f32 = 42.0;
/// The emoji's size inside its cell.
const GLYPH_FRAC: f32 = 0.58;
/// Band height of a group heading ("Smileys & Emotion").
const HEAD_H: f32 = 30.0;
/// Corner radius of a cell's hover halo.
const CELL_RADIUS: f32 = 8.0;

/// What the pointer is over inside the picker.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum EmojiHit {
    #[default]
    None,
    /// The "‹ Back" button — return to the history list.
    Back,
    /// A grid cell, by position in [`EmojiState::shown`].
    Cell(usize),
    /// The search field along the bottom — clicking it takes the keyboard back
    /// after an emoji handed it to the window below.
    Search,
}

/// The picker's state. Lives beside [`crate::clipboard::ClipState`] rather than
/// inside it: it is a mode of the box, but none of the list's machinery.
#[derive(Default)]
pub(crate) struct EmojiState {
    /// Intent: is the picker up? (The wipe eases separately.)
    pub open: bool,
    /// Wipe progress: 0 = the list, 1 = the grid. Smoothstep of `p`.
    pub t: f32,
    /// Linear open progress advanced at a constant rate.
    pub p: f32,
    /// Indices into [`EMOJI`] the grid shows — every emoji, or the search's
    /// matches. The ONE list the draw, the hit-test and the scroll span walk.
    pub shown: Vec<u32>,
    /// Animated / target vertical scroll of the grid.
    pub scroll: f32,
    pub scroll_target: f32,
    /// What the pointer is on.
    pub hit: EmojiHit,
}

/// A laid-out grid: the VISIBLE cells, the visible group headings, and the total
/// stacked height (for the scroll span). One function for the draw and the
/// hit-test, so a click can never land on a cell that isn't where it looks.
struct Layout {
    cells: Vec<(usize, Rect)>,
    heads: Vec<(usize, f32)>,
    total: f32,
}

impl App {
    /// Whether the picker owns the box right now.
    pub(crate) fn emoji_open(&self) -> bool {
        self.emoji.open
    }

    /// Open the picker over the history list (the footer's 🙂 button).
    pub(crate) fn open_emoji(&mut self) {
        self.emoji.open = true;
        self.emoji.scroll = 0.0;
        self.emoji.scroll_target = 0.0;
        // The box's search field now searches EMOJI: a query carried over from
        // the clip list would mean nothing here.
        self.clear_clip_search_query();
        // The picker's footer IS its search field — out from the start, so the
        // grid has one obvious place to type and no buttons to mis-click. It is
        // also the click target that takes the keyboard back after an emoji has
        // been typed into the window below.
        self.arm_clip_search_field();
        self.refilter_emoji();
        self.schedule_clip_frame();
    }

    /// Open the box straight onto the picker with `query` already typed — the
    /// `debug-emoji` verb, so the grid can be captured without a keyboard.
    pub(crate) fn open_emoji_with(&mut self, query: &str) {
        self.open_clip_box();
        if !self.clip.expanded {
            return; // nothing copied yet — the box doesn't open at all
        }
        self.open_emoji();
        if !query.is_empty() {
            self.open_clip_search_with_query(query);
        }
        self.schedule_clip_frame();
    }

    /// Close the picker, back to the history list (and to the footer buttons).
    pub(crate) fn close_emoji(&mut self) {
        self.emoji.open = false;
        self.emoji.hit = EmojiHit::None;
        self.close_clip_search();
        self.schedule_clip_frame();
    }

    /// Reset the picker when the box closes (a fresh box is never mid-pick).
    pub(crate) fn reset_emoji(&mut self) {
        self.emoji.open = false;
        self.emoji.t = 0.0;
        self.emoji.p = 0.0;
        self.emoji.scroll = 0.0;
        self.emoji.scroll_target = 0.0;
        self.emoji.hit = EmojiHit::None;
    }

    /// Re-run the search filter over the emoji table. The only writer of
    /// `EmojiState::shown`; call it on open and after any query edit.
    pub(crate) fn refilter_emoji(&mut self) {
        let needle = self.clip_search_needle();
        self.emoji.shown = if needle.is_empty() {
            (0..EMOJI.len() as u32).collect()
        } else {
            EMOJI
                .iter()
                .enumerate()
                .filter(|(_, e)| emoji_matches(e, &needle))
                .map(|(i, _)| i as u32)
                .collect()
        };
        // A new result set reads from its top.
        self.emoji.scroll = 0.0;
        self.emoji.scroll_target = 0.0;
    }

    /// Group headings only make sense over the whole table — a filtered grid is
    /// a flat list of matches.
    fn emoji_headed(&self) -> bool {
        self.clip_search_needle().is_empty()
    }

    /// The grid's area: the box interior between the back row and the footer
    /// (which keeps its buttons / search field while the picker is up).
    fn emoji_area(&self, rect: Rect) -> Rect {
        let top = rect.y + self.clip_back_seat(rect).h + 2.0 * crate::options::PILL_MARGIN_Y;
        let bottom = rect.y + rect.h - self.clip_footer_h();
        Rect::new(rect.x, top, rect.w, (bottom - top).max(0.0))
    }

    /// Lay the grid out, emitting only what falls inside `area` (the loop still
    /// walks every shown emoji — that is what keeps the stacked height, and so
    /// the scroll span, honest).
    fn emoji_layout(&self, area: Rect) -> Layout {
        let s = self.options_scale();
        let cell = CELL * s;
        let pad = PAD * s;
        let head_h = HEAD_H * s;
        let cols = (((area.w - 2.0 * pad) / cell).floor() as usize).max(1);
        let headed = self.emoji_headed();
        let top0 = area.y + pad - self.emoji.scroll;
        let mut y = top0;
        let mut col = 0usize;
        let mut last_group: Option<u8> = None;
        let mut cells = Vec::new();
        let mut heads = Vec::new();
        let visible = |y: f32, h: f32| y + h > area.y && y < area.y + area.h;
        for (pos, &i) in self.emoji.shown.iter().enumerate() {
            let def = &EMOJI[i as usize];
            if headed && last_group != Some(def.group) {
                if col > 0 {
                    y += cell;
                    col = 0;
                }
                if visible(y, head_h) {
                    heads.push((def.group as usize, y));
                }
                y += head_h;
                last_group = Some(def.group);
            }
            if visible(y, cell) {
                let x = area.x + pad + col as f32 * cell;
                cells.push((pos, Rect::new(x, y, cell, cell)));
            }
            col += 1;
            if col == cols {
                col = 0;
                y += cell;
            }
        }
        if col > 0 {
            y += cell;
        }
        Layout {
            cells,
            heads,
            total: y + pad - top0,
        }
    }

    /// Max scroll (px) of the grid past its visible area.
    pub(crate) fn emoji_scroll_span(&self) -> f32 {
        let area = self.emoji_area(self.clip_rect());
        (self.emoji_layout(area).total - area.h).max(0.0)
    }

    /// A wheel notch over the open picker.
    pub(crate) fn emoji_axis(&mut self, delta: f32) {
        let span = self.emoji_scroll_span();
        self.emoji.scroll_target = (self.emoji.scroll_target + delta).clamp(0.0, span);
        self.schedule_clip_frame();
    }

    /// What the pointer is over in the picker (back button > the field > a cell).
    pub(crate) fn emoji_hit_at(&self, p: (f32, f32)) -> EmojiHit {
        let rect = self.clip_rect();
        if self.clip_back_seat(rect).contains(p) {
            return EmojiHit::Back;
        }
        if self.clip_footer_rect(rect).contains(p) {
            return EmojiHit::Search;
        }
        let area = self.emoji_area(rect);
        if !area.contains(p) {
            return EmojiHit::None;
        }
        for (pos, cr) in self.emoji_layout(area).cells {
            if cr.contains(p) {
                return EmojiHit::Cell(pos);
            }
        }
        EmojiHit::None
    }

    /// Store the pointer's target; returns whether it changed (redraw).
    pub(crate) fn update_emoji_hit(&mut self) -> bool {
        let hit = match self.options_ptr {
            Some(p) if self.emoji.open => self.emoji_hit_at(p),
            _ => EmojiHit::None,
        };
        let changed = hit != self.emoji.hit;
        self.emoji.hit = hit;
        changed
    }

    /// A click inside the picker. Returns whether it consumed it.
    pub(crate) fn emoji_click(&mut self) -> bool {
        match self.emoji.hit {
            EmojiHit::Back => {
                self.close_emoji();
                true
            }
            EmojiHit::Cell(pos) => {
                self.type_emoji_at(pos);
                true
            }
            EmojiHit::Search => {
                self.rearm_clip_keyboard();
                true
            }
            EmojiHit::None => false,
        }
    }

    /// Type the emoji at grid position `pos` into the window underneath.
    pub(crate) fn type_emoji_at(&mut self, pos: usize) {
        let Some(&i) = self.emoji.shown.get(pos) else {
            return;
        };
        let ch = EMOJI[i as usize].ch;
        tracing::debug!("emoji: typing {ch} ({})", EMOJI[i as usize].name);
        // Hand it to our clipboard source, then paste it into the window below
        // — which includes giving the keyboard (and so the clipboard offer)
        // back for the moment it takes. The source thread needs a beat to own
        // the selection first; the focus hand-back covers it.
        self.serve_transient_text(ch);
        self.paste_into_window_below();
        self.schedule_clip_frame();
    }

    /// Draw the picker over the list: an opaque cover, a "‹ Back" button, and
    /// the scrolling grid. The footer (buttons / search field) is drawn by the
    /// box after this, so searching stays available while picking.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_clip_emoji(
        &self,
        scene: &mut Scene,
        rect: Rect,
        e: f32,
        ink: [f32; 4],
        dim_ink: [f32; 4],
        fill: [f32; 4],
    ) {
        let a = (self.emoji.t * e).clamp(0.0, 1.0);
        if a <= 0.001 {
            return;
        }
        let s = self.options_scale();
        // Opaque cover (the renderer draws every label in one late pass, so the
        // list beneath must be skipped, not painted over — see `push_clip_dict`).
        scene.rects.push(RectInst {
            rect,
            radius: crate::clipboard::BOX_RADIUS,
            color: [fill[0], fill[1], fill[2], a],
            glass: 0.0,
            border: 0.0,
        });
        self.push_clip_back(scene, rect, a, self.emoji.hit == EmojiHit::Back, ink);

        let area = self.emoji_area(rect);
        let layout = self.emoji_layout(area);
        if layout.cells.is_empty() {
            scene.labels.push(Label {
                text: crate::i18n::tr("No emoji match").to_owned(),
                pos: (area.x + area.w / 2.0, area.y + (area.h - LINE_PX * s) / 2.0),
                max_w: area.w,
                font_px: FONT_PX * s,
                line_px: LINE_PX * s,
                centered: true,
                dim: false,
                cache: true,
                family: None,
                color: Some([dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * a]),
                clip: Some(area),
            });
            return;
        }
        for (g, y) in layout.heads {
            scene.labels.push(Label {
                text: crate::i18n::tr(GROUPS[g]).to_owned(),
                pos: (area.x + PAD * s + 4.0, y + (HEAD_H * s - LINE_PX * s) / 2.0),
                max_w: area.w - 2.0 * PAD * s,
                font_px: FONT_PX * s * 0.82,
                line_px: LINE_PX * s,
                centered: false,
                dim: false,
                cache: true,
                family: None,
                color: Some([dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * a * 0.9]),
                clip: Some(area),
            });
        }
        for (pos, cr) in layout.cells {
            let hovered = self.emoji.hit == EmojiHit::Cell(pos);
            if hovered {
                let mut wash = self.options_hover_wash();
                wash[3] *= a;
                scene.rects.push(RectInst {
                    rect: cr,
                    radius: CELL_RADIUS * s,
                    color: wash,
                    glass: 0.0,
                    border: 0.0,
                });
            }
            let Some(&i) = self.emoji.shown.get(pos) else {
                continue;
            };
            let gpx = cr.h * GLYPH_FRAC * if hovered { 1.12 } else { 1.0 };
            let cell = if hovered { hover_grow(cr) } else { cr };
            scene.labels.push(Label {
                text: EMOJI[i as usize].ch.to_owned(),
                // Centred on the cell: the emoji font's glyphs sit on the text
                // baseline, so the label's box is nudged up by the slack.
                pos: (cell.x + cell.w / 2.0, cell.y + (cell.h - gpx) / 2.0),
                max_w: cell.w + 8.0,
                font_px: gpx,
                line_px: gpx,
                centered: true,
                dim: false,
                cache: true,
                // Colour comes from naming the family — see the module docs.
                family: Some(EMOJI_FONT),
                color: Some([1.0, 1.0, 1.0, a]),
                clip: Some(area),
            });
        }
    }
}

/// Whether an emoji answers the search `needle`: any WORD of its CLDR name or of
/// gemoji's shortcodes/tags begins with it. The tags are what make "happy" find
/// a face named "grinning".
///
/// Word-prefix, not substring (which is what the clip list wants): searching
/// "cat" for substrings drags in edu**cat**ion 🎓, notifi**cat**ion 🔔 and
/// appli**cat**ion 🈸 ahead of half the cats (seen live, 2026-09-13).
fn emoji_matches(def: &crate::emoji_table::EmojiDef, needle: &str) -> bool {
    word_prefix(def.name, needle) || word_prefix(def.keys, needle)
}

/// Whether any word of `hay` starts with `needle`, case-insensitively. Words
/// break on spaces and the punctuation gemoji uses in names and shortcodes
/// ("flag: St. Kitts", "man_dancing"), so "kitt" and "dancing" both hit.
fn word_prefix(hay: &str, needle: &str) -> bool {
    let pat: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if pat.is_empty() {
        return true;
    }
    hay.split(|c: char| !c.is_alphanumeric()).any(|word| {
        let mut chars = word.chars().flat_map(char::to_lowercase);
        pat.iter().all(|&c| chars.next() == Some(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(needle: &str) -> Vec<&'static str> {
        EMOJI
            .iter()
            .filter(|e| emoji_matches(e, needle))
            .map(|e| e.ch)
            .collect()
    }

    #[test]
    fn table_is_whole_and_its_groups_resolve() {
        assert!(EMOJI.len() > 1500, "the whole gemoji set, not a sample");
        assert!(EMOJI.iter().all(|e| !e.ch.is_empty() && !e.name.is_empty()));
        assert!(
            EMOJI.iter().all(|e| (e.group as usize) < GROUPS.len()),
            "every entry heads under a real section"
        );
    }

    #[test]
    fn keyword_search_finds_what_the_name_does_not() {
        // The whole point of carrying gemoji's tags: 😀 is named "grinning
        // face", and nobody searches for that.
        assert!(find("happy").contains(&"😀"), "keyword hit");
        assert!(find("turtle").contains(&"🐢"), "name hit");
        assert!(find("thumbsup").contains(&"👍"), "shortcode hit");
        assert!(find("thum").contains(&"👍"), "a prefix is enough");
        assert!(find("zzqqx").is_empty());
    }

    #[test]
    fn search_matches_whole_words_only() {
        let cats = find("cat");
        assert!(cats.contains(&"🐱"), "the cats are there");
        // …and nothing that merely *contains* the letters: eduCATion,
        // notifiCATion, appliCATion all used to rank among the cats.
        assert!(!cats.contains(&"🎓"), "graduation cap is not a cat");
        assert!(!cats.contains(&"🔔"), "a notification bell is not a cat");
    }

    #[test]
    fn search_is_case_insensitive() {
        assert_eq!(find("HAPPY"), find("happy"));
    }
}

//! A PAGE IN QUICK LOOK (Max, 2026-10-27: "the quick look is small and
//! tells nothing … it does not even show the full title"): a recent of the
//! dock box's that is a web page (`app_actions::box_peek`) is shown AS THE
//! PAGE — its picture as big as it was found (`web_actions::big_picture`),
//! at most `GROW` its own pixels (Max: "es muy grande, los thumbnails se
//! pixelan", then "un poco más grande"); under it its WHOLE title (up to
//! `TITLE_LINES` lines), how it was seen and how often ("Watched today at
//! 13:02 · 5 times") and its address. In the head, beside "Open in <app>",
//! Copy link and a BIN that takes the recent out of the box (`head`; Delete
//! too). A page with no picture of its own (its favicon on a tile) has the
//! tile small beside the words.
//!
//! One layout (`page_layout`) is what the frame's size (`page_size`) and the
//! draw (`page_words_push`) read; `head` is what the head's draw and the
//! pointer (`page_copy_at`) read.

use crate::content::{GridContent, Rect, RectInst, FONT_BOLD};
use crate::i18n::tr;
use crate::quicklook::{text_label, Said, HEAD};
use crate::App;

/// The words' sizes, their lines and the air round them.
const TITLE_PX: f32 = 20.0;
const TITLE_LINE: f32 = 26.0;
const TITLE_LINES: usize = 3;
const FACTS_PX: f32 = 13.5;
const URL_PX: f32 = 12.0;
const LINE: f32 = 22.0;
const PAD: f32 = 22.0;
/// A page with no picture: its tile beside the words, this big; the frame
/// this wide.
const TILE: f32 = 88.0;
const TILE_W: f32 = 640.0;
/// The frame's narrowest for a page with a picture (a title needs room).
const MIN_W: f32 = 560.0;
/// How much of the screen's width a page's picture may take, at most, and
/// how far past its own pixels it may grow (a little: more shows them).
const SHARE: f32 = 0.55;
const GROW: f32 = 1.3;

/// Where everything of a page stands in its frame.
pub(crate) struct PageLayout {
    /// The picture's room (it is fitted in it), or the tile's square.
    pub pic: Option<Rect>,
    pub tile: Option<Rect>,
    /// The words: where they begin, how wide, the title's lines and the
    /// tops of each line.
    text_x: f32,
    text_w: f32,
    title: Vec<String>,
    title_y: f32,
    facts_y: f32,
    url_y: f32,
}

/// A page's head buttons.
pub(crate) struct Head {
    pub open: Rect,
    pub copy: Rect,
    /// Out of the box's recents.
    pub bin: Rect,
}

/// A page's head buttons in a frame at `r`: Open in <app> (`open_word`)
/// where the pencil and Open stand for a file — no pencil here —, Copy
/// link left of it, the bin left of that. `word_w` measures a button's word.
pub(crate) fn head(r: Rect, open_word: &str, word_w: impl Fn(&str) -> f32) -> Head {
    let close_x = r.x + r.w - 12.0 - 26.0;
    let open_w = (r.w * 0.4).min(240.0).min(word_w(open_word) + 26.0);
    let open = Rect::new(close_x - 8.0 - open_w, r.y + 10.0, open_w, 26.0);
    let copy_w = word_w(tr("Copy link")).max(word_w(tr("Link copied"))) + 26.0;
    let copy = Rect::new(open.x - 8.0 - copy_w, r.y + 10.0, copy_w, 26.0);
    Head { open, copy, bin: Rect::new(copy.x - 8.0 - 32.0, r.y + 10.0, 32.0, 26.0) }
}

/// The bin's can, drawn in strokes in the middle of its button `b` (no
/// font has to have it): the lid and its handle, the body open at the top.
pub(crate) fn bin_can(b: Rect, ink: [f32; 4]) -> Vec<RectInst> {
    let (cx, cy) = ((b.x + b.w / 2.0).round(), (b.y + b.h / 2.0).round());
    let r = |rect: Rect, radius: f32, border: f32| RectInst { rect, radius, color: ink, glass: 0.0, border };
    vec![
        r(Rect::new(cx - 2.5, cy - 7.5, 5.0, 3.0), 1.0, 1.3),
        r(Rect::new(cx - 6.5, cy - 5.0, 13.0, 1.5), 0.75, 0.0),
        r(Rect::new(cx - 5.0, cy - 3.0, 10.0, 10.5), 2.0, 1.3),
        r(Rect::new(cx - 1.95, cy - 0.5, 1.2, 5.5), 0.6, 0.0),
        r(Rect::new(cx + 0.75, cy - 0.5, 1.2, 5.5), 0.6, 0.0),
    ]
}

/// `text` on up to `lines` lines of `room`, broken between words (a word
/// too long for a line is cut as names are), the last cut with "…".
fn wrap(text: &str, room: f32, lines: usize, measure: &mut dyn FnMut(&str) -> f32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut words = text.split_whitespace().peekable();
    while words.peek().is_some() {
        if out.len() + 1 == lines {
            let rest: Vec<&str> = words.collect();
            out.push(crate::desktop::fit_label(&rest.join(" "), room, measure));
            break;
        }
        let mut line = String::new();
        while let Some(w) = words.peek() {
            let next = if line.is_empty() { (*w).to_owned() } else { format!("{line} {w}") };
            if !line.is_empty() && measure(&next) > room - crate::desktop::FIT_SLACK {
                break;
            }
            line = next;
            words.next();
        }
        out.push(crate::desktop::fit_label(&line, room, measure));
    }
    out
}

impl App {
    fn page_measure(&self, text: &str, px: f32, bold: bool) -> f32 {
        let font = bold.then_some(FONT_BOLD);
        match self.desktop_top.renderer.as_ref() {
            Some(r) => r.measure_text(text, px, font),
            None => crate::options::est_text_w(text, px),
        }
    }

    /// The words' height for a text `w` wide (the title's lines counted).
    fn page_words_h(&self, said: &Said, w: f32) -> f32 {
        let n = wrap(&said.title, w, TITLE_LINES, &mut |t| self.page_measure(t, TITLE_PX, true)).len().max(1);
        PAD + n as f32 * TITLE_LINE + 6.0 + 2.0 * LINE + PAD
    }

    /// The frame's body for a page, its picture `pic` (logical px) when it
    /// has one and it is ready; `(mw, mh)` the most there is, `sw` the
    /// screen's width.
    pub(crate) fn page_size(&self, said: &Said, pic: Option<(f32, f32)>, (mw, mh): (f32, f32), sw: f32) -> (f32, f32) {
        match pic.filter(|_| said.picture) {
            Some((pw, ph)) => {
                // As big as it is, at most `SHARE` of the screen — never
                // past its own pixels; the words under it.
                let widest = (sw * SHARE).min(mw);
                let words = self.page_words_h(said, widest - 2.0 * PAD);
                let k = (widest / pw).min((mh - words).max(120.0) / ph).min(GROW);
                let w = (pw * k).max(MIN_W.min(mw));
                (w, ph * k + self.page_words_h(said, w - 2.0 * PAD))
            }
            None if said.picture => {
                // (Not ready: the shape of a video's picture.)
                let w = MIN_W.min(mw);
                (w, w * 9.0 / 16.0 + self.page_words_h(said, w - 2.0 * PAD))
            }
            None => {
                let w = TILE_W.min(mw);
                let words = self.page_words_h(said, w - 3.0 * PAD - TILE);
                (w, words.max(TILE + 2.0 * PAD))
            }
        }
    }

    /// Where everything of the page `said` stands in a frame at `r`.
    pub(crate) fn page_layout(&self, r: Rect, said: &Said) -> PageLayout {
        let body = Rect::new(r.x, r.y + HEAD, r.w, (r.h - HEAD).max(0.0));
        let lay = |text_x: f32, text_w: f32, top: f32, pic: Option<Rect>, tile: Option<Rect>| {
            let title = wrap(&said.title, text_w, TITLE_LINES, &mut |t| self.page_measure(t, TITLE_PX, true));
            let title_y = top + PAD;
            let facts_y = title_y + title.len().max(1) as f32 * TITLE_LINE + 6.0;
            let url_y = facts_y + LINE;
            PageLayout { pic, tile, text_x, text_w, title, title_y, facts_y, url_y }
        };
        if said.picture {
            let words = self.page_words_h(said, body.w - 2.0 * PAD);
            let pic = Rect::new(body.x, body.y, body.w, (body.h - words).max(0.0));
            lay(body.x + PAD, body.w - 2.0 * PAD, pic.y + pic.h, Some(pic), None)
        } else {
            let tile = Rect::new(body.x + PAD, body.y + PAD, TILE, TILE);
            let x = tile.x + TILE + PAD;
            lay(x, body.x + body.w - PAD - x, body.y, None, Some(tile))
        }
    }

    /// The page's words under (or beside) its picture: the title, how it
    /// was seen, its address.
    pub(crate) fn page_words_push(&self, g: &mut GridContent, said: &Said, l: &PageLayout, ink: [f32; 4], dim: [f32; 4]) {
        for (n, line) in l.title.iter().enumerate() {
            g.labels.push(text_label(
                line,
                (l.text_x, l.title_y + n as f32 * TITLE_LINE),
                l.text_w,
                TITLE_PX,
                Some(FONT_BOLD),
                ink,
            ));
        }
        g.labels.push(text_label(&said.facts, (l.text_x, l.facts_y), l.text_w, FACTS_PX, None, dim));
        let url = crate::desktop::fit_name(&said.url, l.text_w, &mut |t| self.page_measure(t, URL_PX, false));
        g.labels.push(text_label(&url, (l.text_x, l.url_y), l.text_w, URL_PX, None, dim));
    }

    /// Whether a click at `at` is on a page's Open in <app>.
    pub(crate) fn page_open_at(&self, at: (f32, f32)) -> bool {
        let Some((up, said)) = self.peek.up.as_ref().and_then(|u| Some((u, u.said.as_ref()?))) else {
            return false;
        };
        head(up.rect, &said.open, |w| self.peek_word_w(w)).open.contains(at)
    }

    /// Whether a click at `at` is on a page's bin.
    pub(crate) fn page_bin_at(&self, at: (f32, f32)) -> bool {
        let Some((up, said)) = self.peek.up.as_ref().and_then(|u| Some((u, u.said.as_ref()?))) else {
            return false;
        };
        head(up.rect, &said.open, |w| self.peek_word_w(w)).bin.contains(at)
    }

    /// The address to copy when a click at `at` is on Copy link.
    pub(crate) fn page_copy_at(&self, at: (f32, f32)) -> Option<String> {
        let up = self.peek.up.as_ref()?;
        let said = up.said.as_ref()?;
        head(up.rect, &said.open, |w| self.peek_word_w(w)).copy.contains(at).then(|| said.url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_breaks_between_words_and_the_last_line_is_cut() {
        // (A letter is 10 px.)
        let mut m = |t: &str| t.chars().count() as f32 * 10.0;
        let lines = wrap("Linux Kernel AI Civil War Anti AI Activist Joins the TAB", 125.0, 3, &mut m);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Linux Kernel");
        assert_eq!(lines[1], "AI Civil War");
        assert!(lines[2].ends_with('…'), "{lines:?}");
        assert_eq!(wrap("Short", 125.0, 3, &mut m), vec!["Short".to_owned()]);
        assert!(wrap("", 125.0, 3, &mut m).is_empty());
    }
}

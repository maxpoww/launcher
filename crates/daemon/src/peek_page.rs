//! A PAGE IN QUICK LOOK (Max, 2026-10-27: "the quick look is small and
//! tells nothing … it does not even show the full title"): a recent of the
//! dock box's that is a web page (`app_actions::box_peek`) is shown AS THE
//! PAGE — its picture as big as it was found (`web_actions::big_picture`),
//! at most `GROW` its own pixels (Max: "es muy grande, los thumbnails se
//! pixelan", then "un poco más grande", twice); under it its WHOLE title (up
//! to `TITLE_LINES` lines), how it was seen and how often ("Watched today at
//! 13:02 · 5 times") and its address. Its head's buttons are THE CARD'S (Max:
//! "use the buttons we use on the card instead of the pills" — its quiet
//! head: an icon alone, a round wash under the pointer): the BIN that takes
//! the recent out of the box (Delete too), Copy link (a check a moment
//! after), Open in its app, × (`head`, `page_head_push`) — in the order
//! Open · Copy link · bin · ×, evenly, each icon's drawing in the middle of
//! its button (Max: "change the order … and align those 4 buttons"); one
//! rested on `TIP_DELAY` says what it does in a pill under it (Max: "add
//! tooltip to the buttons" — `page_tip_follow`, `page_tip_push`). A page
//! with no picture of its own (its favicon on a tile) has the tile small
//! beside the words.
//!
//! One layout (`page_layout`) is what the frame's size (`page_size`) and the
//! draw (`page_words_push`) read; `head` is what the head's draw, its
//! tooltips and the pointer (`page_button_at`) read.

use smithay_client_toolkit::reexports::calloop;

use crate::content::{GridContent, Label, Rect, RectInst, FONT_BOLD};
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
const SHARE: f32 = 0.7;
const GROW: f32 = 1.75;
/// How long the pointer rests on a head button before its tooltip shows.
const TIP_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

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

/// A page's head buttons: round, as the card's head's.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Button {
    /// Out of the box's recents.
    Bin,
    Copy,
    Open,
    Close,
}

/// The card's round button's side, and the air between two.
const BUTTON: f32 = 28.0;
const BUTTON_GAP: f32 = 4.0;

/// A page's head buttons in a frame at `r`, left to right: Open, Copy
/// link, the bin, × — evenly, ending at the head's right.
pub(crate) fn head(r: Rect) -> [(Button, Rect); 4] {
    let y = r.y + ((HEAD - BUTTON) / 2.0).round();
    let right = (r.x + r.w - 10.0).round();
    let at = |n: f32| Rect::new(right - BUTTON - n * (BUTTON + BUTTON_GAP), y, BUTTON, BUTTON);
    [(Button::Open, at(3.0)), (Button::Copy, at(2.0)), (Button::Bin, at(1.0)), (Button::Close, at(0.0))]
}

/// Each button's icon (the Nerd font's), its size, and how far its drawing
/// sits off the middle of its letter's cell (design px, right and down) —
/// taken back out, so the DRAWING is in the middle of the button and the
/// four are as big as each other to the eye. MEASURED on the nested rig
/// with the host's fonts (2026-10-27), as the card's `Icon`s were.
fn icon(b: Button, copied: bool) -> (&'static str, f32, (f32, f32)) {
    match b {
        Button::Open => ("\u{f08e}", 20.0, (0.0, 0.0)),           // fa-external-link
        Button::Copy if copied => ("\u{f00c}", 20.0, (0.0, 0.0)), // fa-check
        Button::Copy => ("\u{f0c1}", 21.5, (0.0, 0.0)),           // fa-link
        Button::Bin => ("\u{f014}", 19.0, (0.0, 0.0)),            // fa-trash-o
        Button::Close => ("\u{f00d}", 18.0, (0.0, 0.0)),          // fa-times (its strokes are heavier)
    }
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

    /// The pointer moved on Quick Look: the head button it is on now starts
    /// its tooltip's wait (a frame is asked for when the wait is over).
    pub(crate) fn page_tip_follow(&mut self) {
        let on = self.peek.up.as_ref().and_then(|u| u.ptr).and_then(|p| self.page_button_at(p));
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        if up.tip.map(|(b, _)| b) == on {
            return;
        }
        up.tip = on.map(|b| (b, std::time::Instant::now()));
        if on.is_some() {
            let timer = calloop::timer::Timer::from_duration(TIP_DELAY + std::time::Duration::from_millis(20));
            if let Err(e) = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
                app.request_desktop_top_draw();
                calloop::timer::TimeoutAction::Drop
            }) {
                tracing::warn!("quick look: cannot wait for a tooltip: {e}");
            }
        }
    }

    /// The tooltip of the head button rested on (`TIP_DELAY` and more):
    /// what it does, in Quick Look's own dark pill under it, inside the
    /// frame at `r` — over the picture (a grid of its own, drawn last).
    pub(crate) fn page_tip_push(&self, clip: Rect, r: Rect, open: f32, copied: bool) -> Option<GridContent> {
        let up = self.peek.up.as_ref()?;
        let said = up.said.as_ref()?;
        let (b, since) = up.tip?;
        if since.elapsed() < TIP_DELAY || up.closing {
            return None;
        }
        let (_, at) = head(r).into_iter().find(|(x, _)| *x == b)?;
        let word = match b {
            Button::Open => said.open.clone(),
            Button::Copy if copied => tr("Link copied").to_owned(),
            Button::Copy => tr("Copy link").to_owned(),
            Button::Bin => tr("Remove from recents").to_owned(),
            Button::Close => tr("Close").to_owned(),
        };
        let w = self.page_measure(&word, 12.0, false) + 22.0;
        let x = (at.x + at.w / 2.0 - w / 2.0).min(r.x + r.w - 8.0 - w).max(r.x + 8.0);
        let pill = Rect::new(x.round(), (at.y + at.h + 6.0).round(), w.round(), 24.0);
        let mut g = GridContent { clip, ..Default::default() };
        g.rects.push(RectInst { rect: pill, radius: 12.0, color: [0.06, 0.07, 0.09, 0.85 * open], glass: 0.0, border: 0.0 });
        g.labels.push(Label {
            centered: true,
            ..text_label(&word, (pill.x + pill.w / 2.0, pill.y + 5.0), w, 12.0, None, [1.0, 1.0, 1.0, open])
        });
        Some(g)
    }

    /// The page's head button a click at `at` is on.
    pub(crate) fn page_button_at(&self, at: (f32, f32)) -> Option<Button> {
        let up = self.peek.up.as_ref().filter(|u| u.said.is_some())?;
        head(up.rect).into_iter().find(|(_, r)| r.contains(at)).map(|(b, _)| b)
    }

    /// THE HEAD'S BUTTONS, the card's way: each its icon alone (the Nerd
    /// font's, at 72 % of the ink, all of it under the pointer), a round wash
    /// under the pointer `ptr`; Copy link a check for a moment after a copy
    /// (`copied`).
    pub(crate) fn page_head_push(
        &self,
        g: &mut GridContent,
        r: Rect,
        (ink, open): ([f32; 4], f32),
        (ptr, copied): (Option<(f32, f32)>, bool),
    ) {
        for (b, rect) in head(r) {
            let hot = ptr.is_some_and(|p| rect.contains(p));
            if hot {
                g.rects.push(RectInst {
                    rect,
                    radius: rect.h / 2.0,
                    color: [ink[0], ink[1], ink[2], 0.09 * open],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            // (At the card's size: the Nerd font's icons fill a mono cell,
            // so their letter is big for their drawing — `card::view::Icon`.)
            let (glyph, px, (dx, dy)) = icon(b, copied);
            let line = (px * 1.3225f32).round();
            g.labels.push(Label {
                centered: true,
                family: Some(crate::options::NERD),
                ..text_label(
                    glyph,
                    (rect.x + rect.w / 2.0 - dx, rect.y + (rect.h - line) / 2.0 - dy),
                    rect.w + 8.0,
                    px,
                    None,
                    [ink[0], ink[1], ink[2], ink[3] * if hot || (copied && b == Button::Copy) { 0.95 } else { 0.72 }],
                )
            });
        }
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

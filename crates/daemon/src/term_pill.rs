//! THE TERMINAL'S PILL on the banner (Max, 2026-11-04: "i want a pill on
//! OPTIONS when there is running stuff on this terminal… with the terminal
//! symbol, hover reveal the tabs to the right (i want the pill at the right
//! of the card pill) when i hover the pill it shows the current tabs on the
//! terminal and the symbol becomes a + clicking opens a new tab").
//!
//! AT REST: one round pill right of the card's pill (and of the copy-link
//! when that is out), the terminal's symbol, while the box's terminal
//! (`term.rs`) has tabs — first only while one ran something, which hid it
//! with the tabs at their prompts ("i dont see it"); what stands right of it
//! moves over.
//!
//! THE POINTER ON IT: its symbol becomes a +, and the tabs come out from
//! behind it to its right as words ("1 ~", "2 cargo"), the ones running
//! something in full ink, the others quieter — as the power pills come out of
//! theirs.
//!
//! A CLICK on the + opens the box as the terminal on a new tab; on a tab's
//! word, on that tab; on the × at a tab's right (Max: "agrega la x en cada
//! pill para poder cerrar las tabs desde el banner sin tener que abrir la
//! caja"), that tab closes — asked first (the × red, a second click) when
//! something runs in it, as the box's own ×.

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, BOND_GAP, FONT_PX, LINE_PX, NERD, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-terminal.
const GLYPH: &str = "\u{f120}";
/// fa-plus: a new tab.
const GLYPH_NEW: &str = "\u{f067}";
/// A tab's ×, and the room it takes at its pill's right (of the pill's
/// height), and the air after it.
const CLOSE: &str = "×";
const CLOSE_W: f32 = 0.8;
const CLOSE_AIR: f32 = 0.4;

/// The pill's state.
#[derive(Debug, Default)]
pub(crate) struct TermPill {
    /// 0 (gone) → 1 (there).
    t: f32,
    /// The hand on it: the tabs come out.
    open: bool,
    /// 0 (behind the round pill) → 1 (out).
    out: f32,
    /// The tabs' words (each with its tab and whether it runs something), and
    /// their widths as measured.
    words: Vec<(u32, String, bool)>,
    words_w: Vec<f32>,
    last: Option<Instant>,
    frame_pending: bool,
}

impl App {
    /// Whether the pill stands: the terminal has tabs (Max, 2026-11-04, the
    /// pill shown only while one ran something: "i dont see it") — or the
    /// hand is on it.
    fn term_pill_on(&self) -> bool {
        !self.term.tabs.is_empty() || self.term_pill.open
    }

    /// The tabs changed (or what runs in them): the words again, measured,
    /// and the pill toward where it should be.
    pub(crate) fn term_pill_sync(&mut self) {
        let words: Vec<(u32, String, bool)> = self.term.tabs.iter().map(|t| (t.n, format!("{} {}", t.n, t.name()), !t.idle())).collect();
        if words != self.term_pill.words {
            let font_px = FONT_PX * self.options_scale();
            if let Some(r) = self.options_renderer.as_mut() {
                self.term_pill.words_w = words.iter().map(|(_, w, _)| r.measure_text(w, font_px, TEXT_FONT)).collect();
            }
            self.term_pill.words = words;
        }
        self.schedule_term_pill_frame();
    }

    /// A tab's pill's width: its word, its × and the pill's air round them.
    fn term_pill_word_w(&self, k: usize) -> f32 {
        let pad = PILL_PAD_X * self.options_scale();
        pad + self.term_pill.words_w.get(k).copied().unwrap_or(24.0) + CLOSE_W * self.options_pill_h() + CLOSE_AIR * pad
    }

    /// Where a tab's × stands in its pill `r`.
    fn term_pill_close_rect(&self, r: Rect) -> Rect {
        let w = CLOSE_W * r.h;
        Rect::new(r.x + r.w - w - CLOSE_AIR * PILL_PAD_X * self.options_scale(), r.y, w, r.h)
    }

    /// How much of the banner right of the card's pill it takes now (what
    /// stands there moves over by it).
    pub(crate) fn term_pill_room(&self) -> f32 {
        let ph = self.options_pill_h();
        let p = &self.term_pill;
        let tabs: f32 = (0..p.words.len()).map(|k| self.term_pill_word_w(k) + BOND_GAP).sum();
        p.t.clamp(0.0, 1.0) * (ph + OPTION_GAP) + p.out.clamp(0.0, 1.0) * tabs
    }

    /// The pills now, the round one at `x`: then the tabs, each as far out as
    /// they are.
    pub(crate) fn term_pill_rects(&self, x: f32, y: f32, ph: f32) -> Vec<(PillId, Rect)> {
        if self.term_pill.t <= 0.01 {
            return Vec::new();
        }
        let mut rects = vec![(PillId::TermPill, Rect::new(x, y, ph, ph))];
        let mut to = x + ph + BOND_GAP;
        for k in 0..self.term_pill.words.len() {
            let w = self.term_pill_word_w(k);
            let tab = Rect::new(lerp(x, to, self.term_pill.out), y, w, ph);
            // (Its × first: the pointer finds the first pill under it.)
            rects.push((PillId::TermTabClose(k as u8), self.term_pill_close_rect(tab)));
            rects.push((PillId::TermTab(k as u8), tab));
            to += w + BOND_GAP;
        }
        rects
    }

    /// Whether a tab's pill is out far enough to be pointed at.
    pub(crate) fn term_pill_reachable(&self, id: PillId) -> bool {
        id == PillId::TermPill || self.term_pill.out > 0.5
    }

    /// Whether the pointer is on the pills — the gaps between them included.
    fn term_pill_ptr_on(&self) -> bool {
        let Some((x, y)) = self.options_ptr else {
            return false;
        };
        if self.term_pill.t < 0.5 || self.stats_covering() || y < 0.0 || y > self.options_bar_h() {
            return false;
        }
        let ph = self.options_pill_h();
        let left = self.term_pill_x();
        let right = self.term_pill_rects(left, 0.0, ph).iter().map(|(_, r)| r.x + r.w).fold(left + ph, f32::max);
        x >= left && x <= right
    }

    /// The pointer moved on the banner (or left it): the tabs come out under
    /// it and go back when it leaves.
    pub(crate) fn update_term_pill_hover(&mut self) -> bool {
        if self.term_pill.t <= 0.01 && !self.term_pill.open {
            return false;
        }
        let on = self.term_pill_ptr_on();
        if on != self.term_pill.open {
            self.term_pill.open = on;
            // (Gone from under the hand: a × asking for a second click asks
            // no more — unless the box, as the terminal, asks it too.)
            if !on && !self.term_on() {
                self.term.closing = None;
            }
            self.schedule_term_pill_frame();
        }
        on
    }

    /// A click: the + opens the box as the terminal on a new tab; a tab's
    /// word, on that tab.
    pub(crate) fn term_pill_click(&mut self, id: PillId) {
        // A tab's ×: that tab closes, here (the box stays as it is).
        if let PillId::TermTabClose(k) = id {
            if let Some(n) = self.term_pill.words.get(usize::from(k)).map(|(n, _, _)| *n) {
                if self.term.closing == Some(n) {
                    self.term.closing = None;
                    self.term_close_now(n);
                } else {
                    self.term_close(n);
                }
            }
            self.draw_options();
            return;
        }
        self.term.closing = None;
        let tab = match id {
            PillId::TermTab(k) => self.term_pill.words.get(usize::from(k)).map(|(n, _, _)| *n),
            _ => None,
        };
        self.term_pill.open = false;
        self.schedule_term_pill_frame();
        self.handle_command(waverunner_proto::Command::Terminal(String::new()));
        match tab {
            Some(n) => self.term_open_on(n),
            None => self.term_open_new(),
        }
    }

    fn schedule_term_pill_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.term_pill.frame_pending, &mut a.term_pill.last), App::tick_term_pill);
    }

    /// One frame of its coming, opening and going.
    fn tick_term_pill(&mut self) {
        let now = Instant::now();
        let dt = self.term_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.term_pill.last = Some(now);
        let ph = self.options_pill_h();
        let span: f32 = (0..self.term_pill.words.len()).map(|k| self.term_pill_word_w(k) + BOND_GAP).sum();
        let on = self.term_pill_on();
        let p = &mut self.term_pill;
        let (t, a) = ease_toward(p.t, f32::from(u8::from(on)), dt, animation::MORPH_RATE, animation::settle_t(ph + OPTION_GAP));
        let (out, b) = ease_toward(p.out, f32::from(u8::from(on && p.open)), dt, animation::MORPH_RATE, animation::settle_t(span.max(1.0)));
        (p.t, p.out) = (t, out);
        self.sync_options_input();
        self.draw_options();
        if a || b {
            self.schedule_term_pill_frame();
        } else {
            self.term_pill.last = None;
        }
    }

    /// One of the pills drawn: the round one (its symbol, a + under the
    /// hand), or a tab's word.
    pub(crate) fn push_term_pill(&self, scene: &mut Scene, rect: Rect, id: PillId) {
        let p = &self.term_pill;
        let shown = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let a = if id == PillId::TermPill { shown } else { shown * ((p.out - 0.15) / 0.6).clamp(0.0, 1.0) };
        if a <= 0.004 {
            return;
        }
        // A tab's ×, on its tab's pill: quiet, in full ink under the hand,
        // red asking for a second click.
        if let PillId::TermTabClose(k) = id {
            let n = p.words.get(usize::from(k)).map(|(n, _, _)| *n);
            let ink = self.options_text_color();
            let c = if n.is_some() && self.term.closing == n {
                // (THE KIT's red, the menus'.)
                crate::kit::danger(crate::kit::ink_is_light(ink))
            } else if self.options_hover == Some(id) {
                ink
            } else {
                [ink[0], ink[1], ink[2], ink[3] * 0.55]
            };
            let s = self.options_scale();
            scene.labels.push(Label {
                text: CLOSE.to_owned(),
                pos: (rect.x + rect.w / 2.0, rect.y + (rect.h - LINE_PX * s) / 2.0),
                max_w: rect.w + 4.0,
                font_px: FONT_PX * s,
                line_px: LINE_PX * s,
                centered: true,
                dim: false,
                cache: true,
                family: TEXT_FONT,
                color: Some([c[0], c[1], c[2], c[3] * a]),
                clip: Some(rect),
            });
            return;
        }
        let bright = self.options_bar_is_bright();
        // (A tab's pill lifts under the hand on its × too.)
        let hovered = self.options_hover == Some(id) || matches!((id, self.options_hover), (PillId::TermTab(k), Some(PillId::TermTabClose(j))) if k == j);
        let r = if hovered { hover_grow(rect) } else { rect };
        push_neumorph(scene, r, r.h / 2.0, bright, a);
        let mut wash = if hovered { self.options_hover_wash() } else { self.options_rest_wash() };
        wash[3] *= a;
        scene.rects.push(RectInst { rect: r, radius: r.h / 2.0, color: wash, glass: 0.0, border: 0.0 });
        // (Labels are drawn in one late pass, over every pill: a tab coming out
        // keeps its word off the pill it comes from behind.)
        let ph = rect.h;
        // (The pills it comes from behind: the round one and the tabs before
        // it — never a ×, which stands inside its own tab.)
        let rects: Vec<(PillId, Rect)> =
            self.term_pill_rects(self.term_pill_x(), rect.y, ph).into_iter().filter(|(i, _)| !matches!(i, PillId::TermTabClose(_))).collect();
        let before = rects.iter().position(|(i, _)| *i == id).and_then(|k| k.checked_sub(1)).map(|k| rects[k].1.x + rects[k].1.w);
        let clip = match before {
            Some(edge) if edge > r.x => Rect::new(edge, r.y, (r.x + r.w - edge).max(0.0), r.h),
            _ => r,
        };
        let s = self.options_scale();
        let ink = self.options_text_color();
        let (text, family, colour) = match id {
            PillId::TermTab(k) => {
                let (word, runs) = p.words.get(usize::from(k)).map_or((String::new(), false), |(_, w, run)| (w.clone(), *run));
                // (A tab at its prompt is quieter than one running something.)
                let c = if runs { ink } else { [ink[0], ink[1], ink[2], ink[3] * 0.6] };
                (word, TEXT_FONT, c)
            }
            _ => ((if p.open { GLYPH_NEW } else { GLYPH }).to_owned(), Some(NERD), ink),
        };
        // (A tab's word stands in what its × leaves.)
        let centre = match id {
            PillId::TermTab(k) => r.x + PILL_PAD_X * s + p.words_w.get(usize::from(k)).copied().unwrap_or(0.0) / 2.0,
            _ => r.x + r.w / 2.0,
        };
        scene.labels.push(Label {
            text,
            pos: (centre, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: r.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family,
            color: Some([colour[0], colour[1], colour[2], colour[3] * a]),
            clip: Some(clip),
        });
    }
}

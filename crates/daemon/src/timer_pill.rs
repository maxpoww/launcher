//! THE SLEEP TIMER ON THE BANNER (Max, 2026-11-04: "ahora que el tiempo y la
//! x estén en el banner (afuera de la caja) cuando está corriendo", then
//! "show the time and the X on the same pill (plus click on the time to
//! edit)"): while the gear box's sleep timer (`sleep_timer.rs`) runs, ONE
//! pill stands on the banner right of the gear — and of the power key's pills
//! while they are up: its countdown, then an X in a circle at its right end.
//! What stands right of it moves over, as it does for the power pills.
//!
//! The pointer on it lights the part under it (the words or the X, the
//! power message's chips' way: the pill does not lift). A click on the X
//! cancels the timer; on the time it opens the gear box with the time asked
//! in its field, the whole minutes written. The box keeps only the hourglass,
//! where the time is set. The pill comes and goes as the timer does, eased.

use std::time::Instant;

use crate::animation::{self, ease_toward};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{push_neumorph, PillId, EDGE_PAD, FONT_PX, LINE_PX, NERD, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-times: cancel (the gear box's own X).
const GLYPH_CANCEL: &str = "\u{f00d}";

/// The pill's state.
#[derive(Debug, Default)]
pub(crate) struct TimerPill {
    /// 0 (gone) → 1 (there).
    t: f32,
    /// The countdown as last shown (kept while the pill goes).
    words: String,
    /// Its width, measured from its shape (`shape`) so the pill holds still
    /// as the seconds go.
    words_w: f32,
    measured: String,
    last: Option<Instant>,
    frame_pending: bool,
}

/// The countdown with every digit a 0: what its width is measured from.
fn shape(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect()
}

impl App {
    /// Where the pill begins: right of the gear and of the power key's pills
    /// as far as they are there.
    fn timer_pill_x(&self) -> f32 {
        EDGE_PAD + self.options_pill_h() + OPTION_GAP + self.power_room()
    }

    /// How wide the words' part is: the words, the pill's air before them
    /// and half of it before the X.
    fn timer_words_part(&self) -> f32 {
        self.timer_pill.words_w + 1.5 * PILL_PAD_X
    }

    fn timer_pill_w(&self) -> f32 {
        self.timer_words_part() + self.options_pill_h()
    }

    /// How much of the banner the pill takes now (what stands right of it
    /// moves over by it).
    pub(crate) fn timer_pill_room(&self) -> f32 {
        self.timer_pill.t.clamp(0.0, 1.0) * (self.timer_pill_w() + OPTION_GAP)
    }

    /// The pill on the banner now.
    pub(crate) fn timer_pill_rects(&self, y: f32, ph: f32) -> Vec<(PillId, Rect)> {
        if self.timer_pill.t <= 0.01 {
            return Vec::new();
        }
        vec![(PillId::Timer, Rect::new(self.timer_pill_x(), y, self.timer_pill_w(), ph))]
    }

    /// The pill's X: the circle at its right end.
    fn timer_x_rect(r: Rect) -> Rect {
        Rect::new(r.x + r.w - r.h, r.y, r.h, r.h)
    }

    /// Whether the pointer stands on the pill's X (else on its words).
    fn timer_ptr_on_x(&self) -> bool {
        let ph = self.options_pill_h();
        let x = Self::timer_x_rect(Rect::new(self.timer_pill_x(), 0.0, self.timer_pill_w(), ph)).x;
        self.options_ptr.is_some_and(|p| p.0 >= x)
    }

    /// It takes the pointer once it is mostly there.
    pub(crate) fn timer_pill_reachable(&self) -> bool {
        self.timer_pill.t > 0.5
    }

    /// A click: on the X the timer cancelled, on the time the time edited.
    pub(crate) fn timer_pill_click(&mut self) {
        if self.timer_ptr_on_x() {
            self.sleep_timer_cancel();
        } else {
            self.timer_pill_edit();
        }
    }

    /// The time edited: the gear box open on its page, the time asked in its
    /// field (which holds the box open until Enter or Escape).
    pub(crate) fn timer_pill_edit(&mut self) {
        let Some(page) = self.stats_page_for(crate::gear::PageKind::Gear) else {
            return;
        };
        self.stats.reveal = true;
        if !(self.stats.open && self.stats_page() == page) {
            self.stats_open_page(page);
        }
        self.sleep_timer_ask();
    }

    /// The timer began, changed, went on a second, or ended: the countdown
    /// read again, the pill eased toward it.
    pub(crate) fn timer_pill_sync(&mut self) {
        if let Some(left) = self.sleep_timer_left() {
            self.timer_pill.words = crate::sleep_timer::countdown(left);
        }
        let shaped = shape(&self.timer_pill.words);
        if shaped != self.timer_pill.measured {
            let font_px = FONT_PX * self.options_scale();
            if let Some(r) = self.options_renderer.as_mut() {
                self.timer_pill.words_w = r.measure_text(&shaped, font_px, TEXT_FONT);
                self.timer_pill.measured = shaped;
            } else {
                // (No renderer yet — the dock starting with a timer running:
                // a guess, measured on the next second.)
                self.timer_pill.words_w = shaped.chars().count() as f32 * font_px * 0.6;
            }
        }
        self.schedule_timer_pill_frame();
        self.sync_options_input();
        self.draw_options();
    }

    fn schedule_timer_pill_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.timer_pill.frame_pending, &mut a.timer_pill.last), App::tick_timer_pill);
    }

    /// One frame of its coming or going.
    fn tick_timer_pill(&mut self) {
        let now = Instant::now();
        let dt = self.timer_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.timer_pill.last = Some(now);
        let span = self.timer_pill_w() + OPTION_GAP;
        let on = f32::from(u8::from(self.sleep_timer_left().is_some()));
        let (t, moving) = ease_toward(self.timer_pill.t, on, dt, animation::MORPH_RATE, animation::settle_t(span));
        self.timer_pill.t = t;
        self.sync_options_input();
        self.draw_options();
        if moving {
            self.schedule_timer_pill_frame();
        } else {
            self.timer_pill.last = None;
        }
    }

    /// Draw the pill: its ground, the part under the pointer lit, the
    /// countdown's words and the X.
    pub(crate) fn push_timer_pill(&self, scene: &mut Scene, r: Rect) {
        let a = ((self.timer_pill.t - 0.15) / 0.6).clamp(0.0, 1.0);
        if a <= 0.004 {
            return;
        }
        let bright = self.options_bar_is_bright();
        let radius = r.h / 2.0;
        push_neumorph(scene, r, radius, bright, a);
        let mut rest = self.options_rest_wash();
        rest[3] *= a;
        scene.rects.push(RectInst { rect: r, radius, color: rest, glass: 0.0, border: 0.0 });
        let x_rect = Self::timer_x_rect(r);
        let words = Rect::new(r.x, r.y, (x_rect.x - r.x).max(0.0), r.h);
        if self.options_hover == Some(PillId::Timer) {
            let lit = if self.timer_ptr_on_x() { x_rect } else { words };
            let mut wash = self.options_hover_wash();
            wash[3] *= a;
            scene.rects.push(RectInst { rect: lit, radius, color: wash, glass: 0.0, border: 0.0 });
        }
        let ink = self.options_text_color();
        let ink = [ink[0], ink[1], ink[2], ink[3] * a];
        let s = self.options_scale();
        scene.labels.push(Label {
            text: self.timer_pill.words.clone(),
            pos: (r.x + PILL_PAD_X, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: words.w,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: false,
            dim: false,
            // (The countdown changes every second: not worth keeping.)
            cache: false,
            family: TEXT_FONT,
            color: Some(ink),
            clip: Some(words),
        });
        scene.labels.push(Label {
            text: GLYPH_CANCEL.to_owned(),
            pos: (x_rect.x + x_rect.w / 2.0, x_rect.y + (x_rect.h - LINE_PX * s) / 2.0),
            max_w: x_rect.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some(ink),
            clip: Some(x_rect),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_width_is_measured_from_the_shape() {
        assert_eq!(shape("39:58"), "00:00");
        assert_eq!(shape("1:05:00"), "0:00:00");
        assert_eq!(shape("39:58"), shape("10:11"));
    }
}

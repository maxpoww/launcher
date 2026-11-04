//! THE SLEEP TIMER ON THE BANNER (Max, 2026-11-04: "ahora que el tiempo y la
//! x estén en el banner (afuera de la caja) cuando está corriendo"): while the
//! gear box's sleep timer (`sleep_timer.rs`) runs, its countdown stands on the
//! banner right of the gear — and of the power key's pills while they are up —
//! with a round X bonded to its right that cancels it. What stands right of
//! them moves over, as it does for the power pills.
//!
//! The countdown is words, not a button (no lift, no click); the X is the
//! button. The box keeps only the hourglass, where the time is set and, while
//! it runs, edited. Both come and go as the timer does, eased.

use std::time::Instant;

use crate::animation::{self, ease_toward};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, BOND_GAP, EDGE_PAD, FONT_PX, LINE_PX, NERD, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-times: cancel (the gear box's own X).
const GLYPH_CANCEL: &str = "\u{f00d}";

/// The pills' state.
#[derive(Debug, Default)]
pub(crate) struct TimerPill {
    /// 0 (gone) → 1 (there).
    t: f32,
    /// The countdown as last shown (kept while the pills go).
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
    /// Where the countdown's pill begins: right of the gear and of the power
    /// key's pills as far as they are there.
    fn timer_pill_x(&self) -> f32 {
        EDGE_PAD + self.options_pill_h() + OPTION_GAP + self.power_room()
    }

    fn timer_words_pill_w(&self) -> f32 {
        self.timer_pill.words_w + 2.0 * PILL_PAD_X
    }

    /// How much of the banner the pills take now (what stands right of them
    /// moves over by it).
    pub(crate) fn timer_pill_room(&self) -> f32 {
        let ph = self.options_pill_h();
        self.timer_pill.t.clamp(0.0, 1.0) * (self.timer_words_pill_w() + BOND_GAP + ph + OPTION_GAP)
    }

    /// The pills on the banner now: the countdown, the X bonded to it.
    pub(crate) fn timer_pill_rects(&self, y: f32, ph: f32) -> Vec<(PillId, Rect)> {
        if self.timer_pill.t <= 0.01 {
            return Vec::new();
        }
        let x = self.timer_pill_x();
        let w = self.timer_words_pill_w();
        vec![(PillId::Timer, Rect::new(x, y, w, ph)), (PillId::TimerCancel, Rect::new(x + w + BOND_GAP, y, ph, ph))]
    }

    /// Only the X takes the pointer, once it is mostly there.
    pub(crate) fn timer_pill_reachable(&self, id: PillId) -> bool {
        id == PillId::TimerCancel && self.timer_pill.t > 0.5
    }

    /// The X clicked: the timer cancelled.
    pub(crate) fn timer_pill_click(&mut self, id: PillId) {
        if id == PillId::TimerCancel {
            self.sleep_timer_cancel();
        }
    }

    /// The timer began, changed, went on a second, or ended: the countdown
    /// read again, the pills eased toward it.
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

    /// One frame of their coming or going.
    fn tick_timer_pill(&mut self) {
        let now = Instant::now();
        let dt = self.timer_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.timer_pill.last = Some(now);
        let span = self.timer_words_pill_w() + BOND_GAP + self.options_pill_h() + OPTION_GAP;
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

    /// Draw one of the pills: its ground, the X's lifted under the pointer;
    /// the countdown's words or the X on it.
    pub(crate) fn push_timer_pill(&self, scene: &mut Scene, rect: Rect, id: PillId) {
        let a = ((self.timer_pill.t - 0.15) / 0.6).clamp(0.0, 1.0);
        if a <= 0.004 {
            return;
        }
        let bright = self.options_bar_is_bright();
        let hovered = id == PillId::TimerCancel && self.options_hover == Some(id);
        let r = if hovered { hover_grow(rect) } else { rect };
        push_neumorph(scene, r, r.h / 2.0, bright, a);
        let mut wash = if hovered { self.options_hover_wash() } else { self.options_rest_wash() };
        wash[3] *= a;
        scene.rects.push(RectInst { rect: r, radius: r.h / 2.0, color: wash, glass: 0.0, border: 0.0 });
        let ink = self.options_text_color();
        let s = self.options_scale();
        let (text, family) = if id == PillId::TimerCancel {
            (GLYPH_CANCEL.to_owned(), Some(NERD))
        } else {
            (self.timer_pill.words.clone(), TEXT_FONT)
        };
        scene.labels.push(Label {
            text,
            pos: (r.x + r.w / 2.0, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: r.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            // (The countdown changes every second: not worth keeping.)
            cache: id == PillId::TimerCancel,
            family,
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(r),
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

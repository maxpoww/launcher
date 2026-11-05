//! THE SLEEP TIMER ON THE BANNER (Max, 2026-11-04: "ahora que el tiempo y la
//! x estén en el banner (afuera de la caja) cuando está corriendo", then
//! "show the time and the X on the same pill (plus click on the time to
//! edit)"): while the gear box's sleep timer (`sleep_timer.rs`) runs and the
//! box is SHUT, ONE pill stands on the banner right of the gear — and of the
//! power key's pills while they are up: its countdown, then an X in a circle
//! at its right end. What stands right of it moves over, as it does for the
//! power pills. While the box is open the timer stands in it as it did (the
//! box's X and countdown); the pill comes as the box shuts (Max: "que pase al
//! banner cuando se cierra la caja") and goes as it opens, eased.
//!
//! The pointer on it lifts and washes the WHOLE pill, as every pill's hover
//! (Max: "el hover effect hacelo en la píldora completa"). A click on the X
//! cancels the timer; on the time the time is EDITED ON THE PILL ITSELF (Max:
//! "la edición después del click en la píldora del banner sea en el banner,
//! no abrir la caja"): the whole minutes left written and picked — the first
//! key typed takes their place —, a caret, a quiet "min" after it; Enter sets
//! it (`sleep_timer_typed`: "45", "1:30", "2h"…), Escape, a click off the pill
//! or the keyboard going elsewhere leave it as it was. The banner holds the
//! keyboard meanwhile (the gear field's grab, `set_timer_keyboard`).

use std::time::Instant;

use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::animation::{self, ease_toward};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, EDGE_PAD, FONT_PX, LINE_PX, NERD, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-times: cancel (the gear box's own X).
const GLYPH_CANCEL: &str = "\u{f00d}";
/// The quiet unit after what is typed on the pill.
const UNIT: &str = " min";
/// The caret's room between what is typed and the unit.
const CARET_AIR: f32 = 3.0;
/// No time typed is longer than this.
const TYPED_MAX: usize = 8;
/// How far the words reach into the X's circle — into its empty side, so the
/// time stands close to the X (Max, 2026-11-04: "más cerca de la x el
/// tiempo"; 4 touched it), in logical px at scale 1: about 4 px of air left
/// between the last digit and the X.
const WORDS_INTO_X: f32 = 1.0;

/// What edits are made of on the pill: digits and the words of a time.
fn typable(c: char) -> bool {
    c.is_ascii_digit() || ":hmin ".contains(c.to_ascii_lowercase())
}

/// The pill's state.
#[derive(Debug, Default)]
pub(crate) struct TimerPill {
    /// The time being typed on the pill, and whether it is still the minutes
    /// it began with (picked: the first key takes their place).
    editing: Option<String>,
    fresh: bool,
    /// The banner holds the keyboard for it.
    keys_held: bool,
    /// What is typed and the quiet " min" after it, measured.
    typed_w: f32,
    unit_w: f32,
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

    /// How wide the words' part is: the pill's air before them and the words,
    /// their end a little into the X's circle (being edited: what is typed,
    /// the caret and " min", never narrower than the countdown).
    fn timer_words_part(&self) -> f32 {
        let tp = &self.timer_pill;
        let words = if tp.editing.is_some() { tp.words_w.max(tp.typed_w + CARET_AIR + tp.unit_w) } else { tp.words_w };
        PILL_PAD_X + words - WORDS_INTO_X * self.options_scale()
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

    /// The time edited on the pill: the whole minutes left written and
    /// picked, the keyboard the banner's.
    pub(crate) fn timer_pill_edit(&mut self) {
        let Some(left) = self.sleep_timer_left() else {
            return;
        };
        if self.timer_pill.editing.is_none() {
            self.timer_pill.editing = Some(crate::sleep_timer::whole_minutes(left).to_string());
            self.timer_pill.fresh = true;
            self.set_timer_keyboard(true);
        }
        self.timer_pill_measure_typed();
        self.sync_options_input();
        self.draw_options();
    }

    /// Whether the pill is being edited (it takes every key).
    pub(crate) fn timer_pill_editing(&self) -> bool {
        self.timer_pill.editing.is_some()
    }

    /// Whether the banner holds the keyboard for the pill.
    pub(crate) fn timer_pill_keys(&self) -> bool {
        self.timer_pill.keys_held
    }

    /// A key while the pill is edited: Enter sets the time, Escape leaves it
    /// as it was, Backspace takes a letter back (all of them while they are
    /// still the minutes it began with), what makes a time is typed.
    pub(crate) fn timer_pill_key(&mut self, keysym: Keysym, utf8: Option<&str>) {
        let Some(mut text) = self.timer_pill.editing.clone() else {
            return;
        };
        match keysym {
            Keysym::Escape => return self.timer_pill_edit_end(),
            Keysym::Return | Keysym::KP_Enter => {
                // (Nothing typed: as it was. Not a time: stays to be put right.)
                if text.trim().is_empty() || self.sleep_timer_typed(&text) {
                    self.timer_pill_edit_end();
                } else {
                    tracing::info!("sleep timer: {text:?} is not a time");
                }
                return;
            }
            Keysym::BackSpace => {
                if self.timer_pill.fresh {
                    text.clear();
                } else {
                    text.pop();
                }
            }
            _ => {
                if self.modifiers.ctrl || self.modifiers.alt {
                    return;
                }
                let typed: String = utf8.unwrap_or("").chars().filter(|c| typable(*c)).collect();
                if typed.is_empty() {
                    return;
                }
                if self.timer_pill.fresh {
                    text.clear();
                }
                text.push_str(&typed);
                text = text.chars().take(TYPED_MAX).collect();
            }
        }
        self.timer_pill.fresh = false;
        self.timer_pill.editing = Some(text);
        self.timer_pill_measure_typed();
        self.sync_options_input();
        self.draw_options();
    }

    /// The edit ends with nothing changed by it (Enter's own change made).
    pub(crate) fn timer_pill_edit_end(&mut self) {
        if self.timer_pill.editing.take().is_none() {
            return;
        }
        self.timer_pill.fresh = false;
        self.set_timer_keyboard(false);
        self.sync_options_input();
        self.draw_options();
    }

    /// The keyboard went elsewhere (a click on a window): the edit is left,
    /// the grab already gone.
    pub(crate) fn timer_pill_keys_left(&mut self) {
        if self.timer_pill.editing.take().is_none() && !self.timer_pill.keys_held {
            return;
        }
        self.timer_pill.keys_held = false;
        if let Some(layer) = &self.options_layer {
            crate::surface::set_interactive(layer, self.options_keys_wanted());
        }
        self.sync_options_input();
        self.draw_options();
    }

    /// Take or release the banner's keyboard for the edit (the bar's surface
    /// is shared: the gear's, the clipboard's and the phone box's grabs).
    fn set_timer_keyboard(&mut self, want: bool) {
        if want == self.timer_pill.keys_held {
            return;
        }
        let others = self.clip.keyboard_held
            || self.gear.keyboard_held
            || self.phone_box_keys()
            || self.desktop.config.as_ref().is_some_and(|c| c.hosted && c.editing);
        if want {
            self.cancel_keyboard_handback(crate::KbSurface::Options);
        } else if !others && !self.keyboard_handback_armed(crate::KbSurface::Options) {
            self.begin_keyboard_handback(crate::KbSurface::Options, None);
        }
        self.timer_pill.keys_held = want;
        tracing::debug!("timer pill: keyboard grab {}", if want { "on" } else { "off" });
        let any = self.options_keys_wanted();
        if let Some(layer) = &self.options_layer {
            crate::surface::set_interactive(layer, any);
        }
    }

    /// Measure what is typed and the unit after it.
    fn timer_pill_measure_typed(&mut self) {
        let text = self.timer_pill.editing.clone().unwrap_or_default();
        let font_px = FONT_PX * self.options_scale();
        if let Some(r) = self.options_renderer.as_mut() {
            self.timer_pill.typed_w = if text.is_empty() { 0.0 } else { r.measure_text(&text, font_px, TEXT_FONT) };
            self.timer_pill.unit_w = r.measure_text(UNIT, font_px, TEXT_FONT);
        } else {
            self.timer_pill.typed_w = text.chars().count() as f32 * font_px * 0.6;
            self.timer_pill.unit_w = UNIT.chars().count() as f32 * font_px * 0.5;
        }
    }

    /// The timer began, changed, went on a second, or ended: the countdown
    /// read again, the pill eased toward it.
    pub(crate) fn timer_pill_sync(&mut self) {
        if let Some(left) = self.sleep_timer_left() {
            self.timer_pill.words = crate::sleep_timer::countdown(left);
        } else {
            // (Ended, or cancelled by its X: nothing is being edited any more.)
            self.timer_pill_edit_end();
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
        let on = f32::from(u8::from(self.sleep_timer_left().is_some() && !self.stats.open));
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

    /// Draw the pill: its ground (lifted and washed whole under the
    /// pointer), the countdown's words and the X.
    pub(crate) fn push_timer_pill(&self, scene: &mut Scene, rect: Rect) {
        let a = ((self.timer_pill.t - 0.15) / 0.6).clamp(0.0, 1.0);
        if a <= 0.004 {
            return;
        }
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(PillId::Timer);
        let r = if hovered { hover_grow(rect) } else { rect };
        let radius = r.h / 2.0;
        push_neumorph(scene, r, radius, bright, a);
        // (Being edited it stays lit: a field, taking the keys.)
        let lit = hovered || self.timer_pill.editing.is_some();
        let mut wash = if lit { self.options_hover_wash() } else { self.options_rest_wash() };
        wash[3] *= a;
        scene.rects.push(RectInst { rect: r, radius, color: wash, glass: 0.0, border: 0.0 });
        let x_rect = Self::timer_x_rect(r);
        let words = Rect::new(r.x, r.y, (x_rect.x - r.x).max(0.0), r.h);
        let ink = self.options_text_color();
        let ink = [ink[0], ink[1], ink[2], ink[3] * a];
        let s = self.options_scale();
        let reach = Rect::new(words.x, words.y, words.w + WORDS_INTO_X * s + 1.0, words.h);
        if let Some(text) = &self.timer_pill.editing {
            self.push_timer_typed(scene, r, reach, text, ink);
        } else {
        scene.labels.push(Label {
            text: self.timer_pill.words.clone(),
            pos: (r.x + PILL_PAD_X, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: reach.w,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: false,
            dim: false,
            // (The countdown changes every second: not worth keeping.)
            cache: false,
            family: TEXT_FONT,
            color: Some(ink),
            clip: Some(reach),
        });
        }
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

impl App {
    /// What is typed on the pill: picked while it is still the minutes it
    /// began with, else a caret after it; the quiet unit after both.
    fn push_timer_typed(&self, scene: &mut Scene, r: Rect, reach: Rect, text: &str, ink: [f32; 4]) {
        let s = self.options_scale();
        let tp = &self.timer_pill;
        let x0 = r.x + PILL_PAD_X;
        let line_h = LINE_PX * s;
        let top = r.y + (r.h - line_h) / 2.0;
        if tp.fresh && !text.is_empty() {
            scene.rects.push(RectInst {
                rect: Rect::new(x0 - 2.0, top, tp.typed_w + 4.0, line_h),
                radius: 3.0 * s,
                color: [ink[0], ink[1], ink[2], ink[3] * 0.22],
                glass: 0.0,
                border: 0.0,
            });
        } else {
            scene.rects.push(RectInst {
                rect: Rect::new(x0 + tp.typed_w + 0.5, top + 2.0, 1.5 * s, line_h - 4.0),
                radius: 0.75 * s,
                color: ink,
                glass: 0.0,
                border: 0.0,
            });
        }
        for (words, at, tone) in [(text, x0, 1.0), (UNIT, x0 + tp.typed_w + CARET_AIR, 0.55)] {
            if words.is_empty() {
                continue;
            }
            scene.labels.push(Label {
                text: words.to_owned(),
                pos: (at, top),
                max_w: reach.w,
                font_px: FONT_PX * s,
                line_px: line_h,
                centered: false,
                dim: false,
                cache: false,
                family: TEXT_FONT,
                color: Some([ink[0], ink[1], ink[2], ink[3] * tone]),
                clip: Some(reach),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_time_is_typed() {
        assert!("0123456789:hmin ".chars().all(typable));
        assert!(typable('H') && typable('M'));
        assert!(!typable('x') && !typable('-') && !typable('.'));
    }

    #[test]
    fn the_width_is_measured_from_the_shape() {
        assert_eq!(shape("39:58"), "00:00");
        assert_eq!(shape("1:05:00"), "0:00:00");
        assert_eq!(shape("39:58"), shape("10:11"));
    }
}

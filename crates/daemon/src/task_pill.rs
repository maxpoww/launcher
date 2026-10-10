//! The TASK pill: the OPTIONS bar's account of the long thing this computer
//! is doing (`tasks.rs`) — what, on the left; how far, as a fill of the pill
//! and a percentage on the right. Max, 2026-10-09: *"a pill on options, with
//! the progress bar, and the percentage as the one we use for the player,
//! the percentage to the right and to the left the process"*.
//!
//! Built from what is on the bar already: the pill's ground is every pill's
//! (neumorph + the rest wash), its bar is the player's track fill (the pill
//! itself filling from the left, the bar's ink at 14%), its words are the
//! player's left-aligned clipped line, its number sits in a slot reserved at
//! its widest ("100%") so the pill does not twitch as the digits change.
//!
//! It stands centred between the player's band and the window's pill (Max,
//! same day), in ground no other OPTION uses, and grows out from its middle:
//! arriving, filling and leaving, it moves nothing else (the Still Bar, by
//! pushing no neighbour); what that ground cannot hold of its words is cut.
//! A box that grows over the bar covers it as it covers anything
//! (`clear_under`). It does nothing when clicked.
//!
//! SEVERAL AT ONCE (Max, same day): the pill shows the last one started and
//! `+N` for the others; a scroll down on it opens ITS BOX — the pill growing
//! down into a list, the others a row each, the way the player's pill grows
//! into the playing list (`playbox.rs`: same morph, same panel) —
//! and a scroll up, or leaving the bar, folds it back. `+N` is a chip in the
//! pill's right cap, the bell's hidden-count chip.

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp, lerp4};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{push_neumorph, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::tasks::Task;
use crate::App;

/// The pill at its widest, in pill-heights. Wider than the player's pill is
/// let be (13): that one scrolls a long name, this one would cut it — at 13
/// "Importing photos from Pixel 8 Pro" lost its last word.
const MAX_W: f32 = 32.0;
/// How much longer than its words and number need the pill is: the fill has
/// further to go, so it is seen to move (Max, 2026-10-09: "make it longer so
/// it goes faster, like another 1/3 longer" — then a quarter longer again).
const STRETCH: f32 = 5.0 / 3.0;
/// A row of the box, in pill-heights.
const ROW_H: f32 = 1.0;
/// The air between one task and the next in the box (Max, 2026-10-10).
const AIR: f32 = 2.0;
/// Between the number and the `+N` chip.
const CHIP_GAP: f32 = 8.0;
/// Between the words and the number.
const GAP: f32 = 10.0;
/// What the number's slot is measured from.
const WIDEST: &str = "100%";

/// The pill's state.
#[derive(Debug, Default)]
pub(crate) struct TaskPill {
    /// The task it shows — kept after the task is gone, for its way out.
    shown: Option<Task>,
    /// How many more are running.
    others: usize,
    /// 0 (not there) → 1 (out).
    t: f32,
    /// The fill, easing after the task's own fraction.
    fill: f32,
    last: Option<Instant>,
    frame_pending: bool,
    /// The words' and the number slot's measured widths, and what was measured.
    label_w: f32,
    pct_w: f32,
    measured: String,
    /// The box of the other tasks: asked open, and how far open it is.
    box_open: bool,
    box_e: f32,
}

impl App {
    /// The words the pill says.
    fn task_pill_label(&self) -> String {
        self.task_pill.shown.as_ref().map(|t| t.label.clone()).unwrap_or_default()
    }

    /// The chip that counts the other tasks (`+2`), and its size: the bell's
    /// hidden-count chip (`notif.rs`), cut the same way. `None` with no others.
    fn task_chip(&self) -> Option<(String, f32, f32)> {
        let n = self.task_pill.others;
        if n == 0 {
            return None;
        }
        let s = self.options_scale();
        let text = format!("+{n}");
        let h = LINE_PX * s;
        let w = (text.chars().count() as f32 * FONT_PX * s * 0.6 + 12.0).max(h);
        Some((text, w, h))
    }

    /// The list of tasks changed (one began, moved on, ended, left): the
    /// pill follows.
    pub(crate) fn tasks_changed(&mut self) {
        if let Some(task) = self.tasks.shown() {
            // A new task starts its fill from nothing, not from the last one's.
            if self.task_pill.shown.as_ref().is_none_or(|was| was.id != task.id) && self.task_pill.t < 0.01 {
                self.task_pill.fill = 0.0;
            }
            self.task_pill.shown = Some(task.clone());
        }
        self.task_pill.others = self.tasks.others();
        let label = self.task_pill_label();
        if label != self.task_pill.measured || self.task_pill.pct_w <= 0.0 {
            let font_px = FONT_PX * self.options_scale();
            if let Some(r) = self.options_renderer.as_mut() {
                self.task_pill.label_w = r.measure_text(&label, font_px, TEXT_FONT);
                self.task_pill.pct_w = r.measure_text(WIDEST, font_px, TEXT_FONT);
                self.task_pill.measured = label;
            }
        }
        self.schedule_task_pill_frame();
        self.draw_options();
    }

    fn schedule_task_pill_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.task_pill.frame_pending, &mut a.task_pill.last), App::tick_task_pill);
    }

    /// One frame of the pill's coming, filling and going.
    fn tick_task_pill(&mut self) {
        let now = Instant::now();
        let dt = self.task_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.task_pill.last = Some(now);
        let out = if self.tasks.is_empty() { 0.0 } else { 1.0 };
        let span = self.task_pill_full_w();
        let (t, coming) = ease_toward(self.task_pill.t, out, dt, animation::MORPH_RATE, animation::settle_t(span));
        self.task_pill.t = t;
        let want = self.task_pill.shown.as_ref().map_or(0.0, Task::fraction);
        let (fill, filling) = ease_toward(self.task_pill.fill, want, dt, animation::MORPH_RATE, animation::settle_t(span));
        self.task_pill.fill = fill;
        // Nothing left to list: the box folds by itself.
        if self.tasks.others() == 0 {
            self.task_pill.box_open = false;
        }
        let open = f32::from(u8::from(self.task_pill.box_open));
        let (box_e, boxing) = ease_toward(self.task_pill.box_e, open, dt, animation::MORPH_RATE, animation::SETTLE_ALPHA);
        self.task_pill.box_e = box_e;
        if coming || boxing {
            self.sync_options_input();
        }
        self.draw_options();
        // (While the box is open its rows fill as their tasks go.)
        if coming || filling || boxing || (self.task_pill.box_open && !self.tasks.is_empty()) {
            self.schedule_task_pill_frame();
        } else {
            self.task_pill.last = None;
            if out == 0.0 {
                self.task_pill.shown = None;
            }
        }
    }

    /// The pill's width when nothing holds it in: its words and its number,
    /// up to `MAX_W`.
    fn task_pill_full_w(&self) -> f32 {
        let chip = self.task_chip().map_or(0.0, |(_, w, _)| w + CHIP_GAP);
        let wanted = 2.0 * PILL_PAD_X + self.task_pill.label_w + GAP + self.task_pill.pct_w;
        (wanted * STRETCH + chip).min(self.options_pill_h() * MAX_W)
    }

    /// Where the pill is on the bar now (`None`: it is not there): centred
    /// in the ground between the left band's end (`band_right`) and the
    /// window's pill (`window_left`), growing out from its middle, its words
    /// giving way if that ground is narrow. With no ground there at all (no
    /// window pill, or a very long title) it stands left of the bell.
    pub(crate) fn task_pill_rect(&self, band_right: f32, window_left: f32, y: f32, ph: f32) -> Option<Rect> {
        let t = self.task_pill.t;
        if t <= 0.01 || self.task_pill.shown.is_none() {
            return None;
        }
        let number = self.task_pill.pct_w + 2.0 * PILL_PAD_X;
        let ground = window_left - band_right - 2.0 * OPTION_GAP;
        if window_left.is_finite() && ground >= number {
            let full = self.task_pill_full_w().min(ground).max(number);
            let w = lerp(ph.min(full), full, t);
            let centre = (band_right + window_left) / 2.0;
            return Some(Rect::new(centre - w / 2.0, y, w, self.task_box_h(ph)));
        }
        let right = self.options_clock_rest_left() - OPTION_GAP - ph - OPTION_GAP;
        let w = lerp(ph, self.task_pill_full_w().max(ph), t);
        Some(Rect::new(right - w, y, w, self.task_box_h(ph)))
    }

    /// The pill's height now: its band, and under it as much of the box as
    /// is open — a row for each of the other tasks.
    fn task_box_h(&self, ph: f32) -> f32 {
        lerp(ph, ph + self.tasks.others() as f32 * (ph * ROW_H + AIR), self.task_pill.box_e)
    }

    /// A scroll over the pill: down opens the box of the other tasks, up
    /// folds it (the playing box's gesture and its sign).
    pub(crate) fn task_pill_axis(&mut self, value: f32) {
        // A scroll's end arrives as an axis event of zero: only a push counts.
        if value.abs() < crate::options::SCROLL_DEADZONE {
            return;
        }
        let open = value < 0.0 && self.tasks.others() > 0;
        if open != self.task_pill.box_open {
            self.task_pill.box_open = open;
            self.schedule_task_pill_frame();
            self.sync_options_input();
            self.draw_options();
        }
    }

    /// The box is open (the bar's pointer region reaches down over it).
    pub(crate) fn task_box_open(&self) -> bool {
        self.task_pill.box_open
    }

    /// Fold the box (the pointer left the bar: the visit is over).
    pub(crate) fn task_box_close(&mut self) {
        if self.task_pill.box_open {
            self.task_pill.box_open = false;
            self.schedule_task_pill_frame();
        }
    }

    /// How far down the pointer region must reach while the box is open.
    pub(crate) fn task_box_input_bottom(&self) -> f32 {
        let ph = self.options_pill_h();
        crate::options::PILL_MARGIN_Y + ph + self.tasks.others() as f32 * (ph * ROW_H + AIR)
    }

    /// Draw it: the ground every pill has, the fill, the words, the number.
    pub(crate) fn push_task_pill(&self, scene: &mut Scene, rect: Rect) {
        let Some(task) = self.task_pill.shown.as_ref() else {
            return;
        };
        let s = self.options_scale();
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);
        let a = ((self.task_pill.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let ink = self.options_text_color();
        // The band is the pill; under it, the box. The player's morph: the
        // corner runs from the stadium to the box's, the fill from the pill's
        // wash to the panel's colour, opacity leading the height.
        let ph = self.options_pill_h();
        let e = self.task_pill.box_e;
        let solid = 1.0 - (1.0 - e).powi(3);
        let band = Rect::new(rect.x, rect.y, rect.w, ph.min(rect.h));
        let radius = lerp(ph / 2.0, crate::clipboard::BOX_RADIUS, e);
        push_neumorph(scene, rect, radius, self.options_bar_is_bright(), a);
        let wash = self.options_rest_wash();
        let (panel, box_ink) = self.clip_box_surface();
        scene.rects.push(RectInst {
            rect,
            radius,
            color: lerp4(
                [wash[0], wash[1], wash[2], wash[3] * a],
                [panel[0], panel[1], panel[2], self.box_panel_alpha() * a],
                solid,
            ),
            glass: 0.0,
            border: 0.0,
        });
        let ink = lerp4(ink, box_ink, solid);
        // The player's track fill: the pill itself, filling from the left.
        let fill = self.task_pill.fill.clamp(0.0, 1.0);
        if fill > 0.0 {
            // (Round-ended always: the box's corner is not the bar's end.)
            let cap = band.h / 2.0;
            let fw = (band.w * fill).max(2.0 * cap).min(band.w);
            scene.rects.push(RectInst {
                rect: Rect::new(band.x, band.y, fw, band.h),
                radius: cap,
                color: [ink[0], ink[1], ink[2], 0.14 * a],
                glass: 0.0,
                border: 0.0,
            });
        }
        self.push_task_rows(scene, rect, solid, ink);
        let rect = band;
        let ty = rect.y + (rect.h - line_px) / 2.0;
        let slot = self.task_pill.pct_w;
        // The `+N` chip nests in the pill's right cap, as the bell's does, and
        // fades as the box opens (the others are then in plain sight); the
        // number stands left of it.
        let chip = self.task_chip();
        let chip_x = chip.as_ref().map(|(_, w, h)| rect.x + rect.w - (ph - h) / 2.0 - w);
        // (As the box opens and the chip goes, the number slides to the edge,
        // into line with the rows' numbers under it.)
        let at_edge = rect.x + rect.w - PILL_PAD_X - slot;
        let slot_x = match chip_x {
            Some(x) => lerp(x - CHIP_GAP - slot, at_edge, e),
            None => at_edge,
        };
        if let (Some((text, w, h)), Some(x)) = (chip, chip_x) {
            let fade = (1.0 - e) * a;
            if fade > 0.004 {
                let cr = Rect::new(x, rect.y + (rect.h - h) / 2.0, w, h);
                let amber = crate::notif::AMBER;
                scene.rects.push(RectInst {
                    rect: cr,
                    radius: h / 2.0,
                    color: [amber[0], amber[1], amber[2], 0.2 * fade],
                    glass: 0.0,
                    border: 0.0,
                });
                scene.labels.push(Label {
                    text,
                    pos: (cr.x + cr.w / 2.0, cr.y + (cr.h - line_px) / 2.0),
                    max_w: cr.w + 4.0,
                    font_px,
                    line_px,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: TEXT_FONT,
                    color: Some([ink[0], ink[1], ink[2], fade]),
                    clip: Some(cr),
                });
            }
        }
        let color = Some([ink[0], ink[1], ink[2], ink[3] * a]);
        // The words, left, cut where the number's slot begins.
        let words_w = (slot_x - GAP - (rect.x + PILL_PAD_X)).max(0.0);
        if words_w > 1.0 {
            scene.labels.push(Label {
                text: self.task_pill_label(),
                pos: (rect.x + PILL_PAD_X, ty),
                max_w: self.task_pill.label_w.max(words_w),
                font_px,
                line_px,
                centered: false,
                dim: false,
                cache: false,
                clip: Some(Rect::new(rect.x + PILL_PAD_X, rect.y, words_w, rect.h)),
                family: TEXT_FONT,
                color,
            });
        }
        // The number, in its slot.
        scene.labels.push(Label {
            text: format!("{}%", task.percent()),
            pos: (slot_x + slot / 2.0, ty),
            max_w: slot.max(1.0),
            font_px,
            line_px,
            centered: true,
            dim: false,
            cache: false,
            clip: Some(rect),
            family: TEXT_FONT,
            color,
        });
    }

    /// The box's rows: the other tasks, each a line of its own like the band
    /// above them — its words, its number, and its fill a round-ended bar as
    /// the band's is — with `AIR` between one and the next. A row is drawn
    /// once the opening box has room for all of it.
    fn push_task_rows(&self, scene: &mut Scene, rect: Rect, solid: f32, ink: [f32; 4]) {
        if self.task_pill.box_e < 0.01 {
            return;
        }
        let s = self.options_scale();
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);
        let ph = self.options_pill_h();
        let row_h = ph * ROW_H;
        let radius = row_h / 2.0;
        let slot = self.task_pill.pct_w;
        let color = Some([ink[0], ink[1], ink[2], ink[3] * solid]);
        for (idx, task) in self.tasks.rest().iter().enumerate() {
            let rr = Rect::new(rect.x, rect.y + ph + AIR + idx as f32 * (row_h + AIR), rect.w, row_h);
            if rr.y + rr.h > rect.y + rect.h + 0.5 {
                break;
            }
            let fraction = task.fraction();
            if fraction > 0.0 {
                let fw = (rr.w * fraction).max(2.0 * radius).min(rr.w);
                scene.rects.push(RectInst {
                    rect: Rect::new(rr.x, rr.y, fw, rr.h),
                    radius,
                    color: [ink[0], ink[1], ink[2], 0.14 * solid],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            let ty = rr.y + (row_h - line_px) / 2.0;
            let slot_x = rr.x + rr.w - PILL_PAD_X - slot;
            let words_w = (slot_x - GAP - (rr.x + PILL_PAD_X)).max(1.0);
            scene.labels.push(Label {
                text: task.label.clone(),
                pos: (rr.x + PILL_PAD_X, ty),
                max_w: 4096.0,
                font_px,
                line_px,
                centered: false,
                dim: false,
                cache: false,
                clip: Some(Rect::new(rr.x + PILL_PAD_X, rr.y, words_w, rr.h)),
                family: TEXT_FONT,
                color,
            });
            scene.labels.push(Label {
                text: format!("{}%", task.percent()),
                pos: (slot_x + slot / 2.0, ty),
                max_w: slot.max(1.0),
                font_px,
                line_px,
                centered: true,
                dim: false,
                cache: false,
                clip: Some(rr),
                family: TEXT_FONT,
                color,
            });
        }
    }
}

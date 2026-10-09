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

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{push_neumorph, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::tasks::Task;
use crate::App;

/// The pill at its widest, in pill-heights. Wider than the player's pill is
/// let be (13): that one scrolls a long name, this one would cut it — at 13
/// "Importing photos from Pixel 8 Pro" lost its last word.
const MAX_W: f32 = 24.0;
/// How much longer than its words and number need the pill is: the fill has
/// further to go, so it is seen to move (Max, 2026-10-09: "make it longer so
/// it goes faster, like another 1/3 longer").
const STRETCH: f32 = 4.0 / 3.0;
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
}

impl App {
    /// The words the pill says.
    fn task_pill_label(&self) -> String {
        let Some(task) = self.task_pill.shown.as_ref() else {
            return String::new();
        };
        match self.task_pill.others {
            0 => task.label.clone(),
            n => format!("{}  +{n}", task.label),
        }
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
        if coming {
            self.sync_options_input();
        }
        self.draw_options();
        if coming || filling {
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
        let wanted = 2.0 * PILL_PAD_X + self.task_pill.label_w + GAP + self.task_pill.pct_w;
        (wanted * STRETCH).min(self.options_pill_h() * MAX_W)
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
            return Some(Rect::new(centre - w / 2.0, y, w, ph));
        }
        let right = self.options_clock_rest_left() - OPTION_GAP - ph - OPTION_GAP;
        let w = lerp(ph, self.task_pill_full_w().max(ph), t);
        Some(Rect::new(right - w, y, w, ph))
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
        let radius = rect.h / 2.0;
        push_neumorph(scene, rect, radius, self.options_bar_is_bright(), a);
        let wash = self.options_rest_wash();
        scene.rects.push(RectInst {
            rect,
            radius,
            color: [wash[0], wash[1], wash[2], wash[3] * a],
            glass: 0.0,
            border: 0.0,
        });
        // The player's track fill: the pill itself, filling from the left.
        let fill = self.task_pill.fill.clamp(0.0, 1.0);
        if fill > 0.0 {
            let fw = (rect.w * fill).max(2.0 * radius).min(rect.w);
            scene.rects.push(RectInst {
                rect: Rect::new(rect.x, rect.y, fw, rect.h),
                radius,
                color: [ink[0], ink[1], ink[2], 0.14 * a],
                glass: 0.0,
                border: 0.0,
            });
        }
        let ty = rect.y + (rect.h - line_px) / 2.0;
        let slot = self.task_pill.pct_w;
        let slot_x = rect.x + rect.w - PILL_PAD_X - slot;
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
}

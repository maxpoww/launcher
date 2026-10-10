//! The TASK pill: the OPTIONS bar's account of the long thing this computer
//! is doing (`tasks.rs`) — what, on the left; how far, as a fill of the pill
//! and a percentage on the right. Max, 2026-10-09: *"a pill on options, with
//! the progress bar, and the percentage as the one we use for the player,
//! the percentage to the right and to the left the process"*. Its look and
//! conduct were settled on a mockup (`~/task-pill-mockup`, 2026-10-10).
//!
//! Built from what is on the bar already: the pill's ground is every pill's
//! (neumorph + the rest wash), its bar is the player's track fill (the pill
//! itself filling from the left, the bar's ink at 14%), its words are the
//! player's left-aligned clipped line, its number sits in a slot reserved at
//! its widest ("100%") so the pill does not twitch as the digits change, and
//! its two chips are the bell's hidden-count chip.
//!
//! WHERE: centred between the player's band and the window's pill, in ground
//! no other OPTION uses, growing out from its middle: arriving, filling and
//! leaving, it moves nothing else (the Still Bar, by pushing no neighbour);
//! what that ground cannot hold of its words is cut. With no such ground it
//! stands left of the bell. A box that grows over the bar covers it as it
//! covers anything (`clear_under`).
//!
//! FROM THE RIGHT, in the pill: `+N` (the other tasks running), `Cancel`
//! (ends the task it stands on), the number.
//!
//! SEVERAL AT ONCE: the pill shows the last one started; a scroll down on it
//! opens ITS BOX — the pill growing down into a list, the others a row each
//! with its own fill, number and Cancel, the way the player's pill grows
//! into the playing list (`playbox.rs`: same morph, same panel) — and a
//! scroll up, or leaving the bar, folds it back.
//!
//! COLLAPSED: a click on the pill (not on a chip) cuts it down to its number
//! and `+N`, nothing else — no words, no fill. While it is collapsed the
//! pointer coming onto it shows it whole, and leaving puts it back; a click
//! on it while whole keeps it whole. Just collapsed, it stays small under
//! the hand that did it until that hand has left and come back.

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp, lerp4};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{push_neumorph, PillId, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::tasks::Task;
use crate::App;

/// The pill at its widest, in pill-heights. Wider than the player's pill is
/// let be (13): that one scrolls a long name, this one would cut it.
const MAX_W: f32 = 40.0;
/// How much longer than its words and number need the pill is: the fill has
/// further to go, so it is seen to move (Max: "make it longer so it goes
/// faster"; settled at this on the mockup).
const STRETCH: f32 = 1.9;
/// A row of the box, in pill-heights.
const ROW_H: f32 = 1.0;
/// The air between one task and the next in the box.
const AIR: f32 = 3.0;
/// Between the words and the number.
const GAP: f32 = 10.0;
/// Between the number, `Cancel` and `+N`.
const CHIP_GAP: f32 = 8.0;
/// The collapsed pill: either side of its number, and between it and `+N`.
const SMALL_PAD: f32 = 7.0;
const SMALL_GAP: f32 = 5.0;
/// What the number's slot is measured from.
const WIDEST: &str = "100%";
const CANCEL: &str = "Cancel";

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
    /// Measured widths: the words, the number's slot, the word on the cancel
    /// chip, the number as it reads now — and what was measured.
    label_w: f32,
    pct_w: f32,
    cancel_w: f32,
    num_w: f32,
    measured: String,
    num_measured: String,
    /// The box of the other tasks: asked open, and how far open it is.
    box_open: bool,
    box_e: f32,
    /// Cut down to its number by a click; shown whole meanwhile by the
    /// pointer (`peek`), unless that pointer is the one that just collapsed
    /// it and has not left yet (`held`). `small` eases 0 (whole) → 1.
    collapsed: bool,
    peek: bool,
    held: bool,
    small: f32,
}

/// Where the band's parts are.
struct Band {
    /// The `+N` chip and what it says.
    more: Option<(Rect, String)>,
    /// The `Cancel` chip (none on a task that is over).
    cancel: Option<Rect>,
    /// The number's left edge and the width it is drawn in.
    number: (f32, f32),
    /// Where the words must stop.
    words_right: f32,
}

impl App {
    /// The words the pill says.
    fn task_pill_label(&self) -> String {
        self.task_pill.shown.as_ref().map(|t| t.label.clone()).unwrap_or_default()
    }

    /// A chip's height (the bell's: one line of the bar's text).
    fn task_chip_h(&self) -> f32 {
        LINE_PX * self.options_scale()
    }

    /// The `+N` chip's words and width; `None` with no other task running.
    fn task_more(&self) -> Option<(String, f32)> {
        let n = self.task_pill.others;
        if n == 0 {
            return None;
        }
        let text = format!("+{n}");
        let w = (text.chars().count() as f32 * FONT_PX * self.options_scale() * 0.6 + 12.0).max(self.task_chip_h());
        Some((text, w))
    }

    /// The `Cancel` chip's width.
    fn task_cancel_w(&self) -> f32 {
        self.task_pill.cancel_w + 12.0
    }

    /// Whether the pill is (heading for) its small self.
    fn task_pill_small(&self) -> bool {
        self.task_pill.collapsed && !self.task_pill.peek
    }

    /// Measure what the pill writes, where it changed.
    fn task_pill_measure(&mut self) {
        let label = self.task_pill_label();
        let number = self.task_pill.shown.as_ref().map(|t| format!("{}%", t.percent())).unwrap_or_default();
        let font_px = FONT_PX * self.options_scale();
        let Some(r) = self.options_renderer.as_mut() else {
            return;
        };
        if label != self.task_pill.measured || self.task_pill.pct_w <= 0.0 {
            self.task_pill.label_w = r.measure_text(&label, font_px, TEXT_FONT);
            self.task_pill.pct_w = r.measure_text(WIDEST, font_px, TEXT_FONT);
            self.task_pill.cancel_w = r.measure_text(CANCEL, font_px, TEXT_FONT);
            self.task_pill.measured = label;
        }
        if number != self.task_pill.num_measured {
            self.task_pill.num_w = r.measure_text(&number, font_px, TEXT_FONT);
            self.task_pill.num_measured = number;
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
        self.task_pill_measure();
        self.schedule_task_pill_frame();
        self.draw_options();
    }

    fn schedule_task_pill_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.task_pill.frame_pending, &mut a.task_pill.last), App::tick_task_pill);
    }

    /// One frame of the pill's coming, filling, collapsing and going.
    fn tick_task_pill(&mut self) {
        let now = Instant::now();
        let dt = self.task_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.task_pill.last = Some(now);
        let out = if self.tasks.is_empty() { 0.0 } else { 1.0 };
        let span = self.task_pill_full_w();
        let settle = animation::settle_t(span);
        let (t, coming) = ease_toward(self.task_pill.t, out, dt, animation::MORPH_RATE, settle);
        self.task_pill.t = t;
        let want = self.task_pill.shown.as_ref().map_or(0.0, Task::fraction);
        let (fill, filling) = ease_toward(self.task_pill.fill, want, dt, animation::MORPH_RATE, settle);
        self.task_pill.fill = fill;
        let small = f32::from(u8::from(self.task_pill_small()));
        let (small, shrinking) = ease_toward(self.task_pill.small, small, dt, animation::MORPH_RATE, settle);
        self.task_pill.small = small;
        // Nothing left to list, or nothing to list it under: the box folds.
        if self.tasks.others() == 0 || self.task_pill_small() {
            self.task_pill.box_open = false;
        }
        let open = f32::from(u8::from(self.task_pill.box_open));
        let (box_e, boxing) = ease_toward(self.task_pill.box_e, open, dt, animation::MORPH_RATE, animation::SETTLE_ALPHA);
        self.task_pill.box_e = box_e;
        if coming || boxing || shrinking {
            self.sync_options_input();
        }
        self.draw_options();
        // (While the box is open its rows fill as their tasks go.)
        if coming || filling || boxing || shrinking || (self.task_pill.box_open && !self.tasks.is_empty()) {
            self.schedule_task_pill_frame();
        } else {
            self.task_pill.last = None;
            if out == 0.0 {
                // Gone: the next one starts whole.
                self.task_pill.shown = None;
                self.task_pill.collapsed = false;
                self.task_pill.peek = false;
                self.task_pill.held = false;
                self.task_pill.small = 0.0;
            }
        }
    }

    /// The pill's width when nothing holds it in: its words and its number
    /// stretched, and its chips, up to `MAX_W`.
    fn task_pill_full_w(&self) -> f32 {
        let more = self.task_more().map_or(0.0, |(_, w)| w + CHIP_GAP);
        let cancel = self.task_cancel_w() + CHIP_GAP;
        let wanted = 2.0 * PILL_PAD_X + self.task_pill.label_w + GAP + self.task_pill.pct_w;
        (wanted * STRETCH + cancel + more).min(self.options_pill_h() * MAX_W)
    }

    /// The pill's width collapsed: cut to its number, and `+N` after it.
    fn task_pill_small_w(&self) -> f32 {
        let ph = self.options_pill_h();
        match self.task_more() {
            Some((_, w)) => SMALL_PAD + self.task_pill.num_w + SMALL_GAP + w + (ph - self.task_chip_h()) / 2.0,
            None => (self.task_pill.num_w + 2.0 * SMALL_PAD).max(ph),
        }
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
        let small_w = self.task_pill_small_w();
        if window_left.is_finite() && ground >= number {
            let full = self.task_pill_full_w().min(ground).max(number);
            let whole = lerp(full, small_w.min(full), self.task_pill.small);
            let w = lerp(ph.min(whole), whole, t);
            let centre = (band_right + window_left) / 2.0;
            return Some(Rect::new(centre - w / 2.0, y, w, self.task_box_h(ph)));
        }
        let right = self.options_clock_rest_left() - OPTION_GAP - ph - OPTION_GAP;
        let whole = lerp(self.task_pill_full_w().max(ph), small_w, self.task_pill.small);
        let w = lerp(ph.min(whole), whole, t);
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
        let open = value < 0.0 && self.tasks.others() > 0 && !self.task_pill_small();
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

    /// The pointer left the bar: the visit is over — the box folds, and a
    /// collapsed pill that was being shown whole is small again.
    pub(crate) fn task_box_close(&mut self) {
        let was = (self.task_pill.box_open, self.task_pill.peek);
        self.task_pill.box_open = false;
        self.task_pill.peek = false;
        self.task_pill.held = false;
        if was != (false, false) {
            self.schedule_task_pill_frame();
        }
    }

    /// The pointer came onto the pill or went off it (called on every hover
    /// move over the bar). Whether the pointer is on it.
    pub(crate) fn update_task_hover(&mut self) -> bool {
        let on = self.options_hover == Some(PillId::Task);
        let peek = on && self.task_pill.collapsed && !self.task_pill.held;
        if !on {
            self.task_pill.held = false;
        }
        if peek != self.task_pill.peek {
            self.task_pill.peek = peek;
            self.schedule_task_pill_frame();
        }
        on
    }

    /// How far down the pointer region must reach while the box is open.
    pub(crate) fn task_box_input_bottom(&self) -> f32 {
        let ph = self.options_pill_h();
        crate::options::PILL_MARGIN_Y + ph + self.tasks.others() as f32 * (ph * ROW_H + AIR)
    }

    /// A click on the pill: on a `Cancel`, that task is over; anywhere else
    /// on its band, it collapses — or, collapsed and shown whole by this
    /// very pointer, stays whole.
    pub(crate) fn task_pill_click(&mut self) {
        let (Some(ptr), Some(rect)) = (self.options_ptr, self.task_pill_drawn()) else {
            return;
        };
        let ph = self.options_pill_h();
        let band = Rect::new(rect.x, rect.y, rect.w, ph.min(rect.h));
        let parts = self.task_band(band);
        let cancelled = if parts.cancel.is_some_and(|c| c.contains(ptr)) && self.task_pill.small < 0.5 {
            self.task_pill.shown.as_ref().map(|t| t.id)
        } else {
            self.task_rows(rect)
                .into_iter()
                .find(|(_, row)| self.task_row_cancel(*row).contains(ptr))
                .map(|(id, _)| id)
        };
        if let Some(id) = cancelled {
            tracing::info!("tasks: #{id} cancelled from the bar");
            if self.tasks.cancel(id) {
                self.tasks_changed();
            }
            return;
        }
        // Only the band collapses it: a click between two rows is nothing.
        if ptr.1 > band.y + band.h {
            return;
        }
        self.task_pill.collapsed = !self.task_pill.collapsed;
        self.task_pill.peek = false;
        // Small now under the hand that asked: it stays so until that hand
        // has gone and come back.
        self.task_pill.held = self.task_pill.collapsed;
        self.schedule_task_pill_frame();
        self.draw_options();
    }

    /// The pill driven without a pointer (`debug-desktop "taskbox …"`).
    pub(crate) fn task_pill_debug(&mut self, what: &str) -> String {
        match what {
            "open" => self.task_pill_axis(-1.0),
            "close" => self.task_pill_axis(1.0),
            // A click on the band.
            "collapse" => {
                self.task_pill.collapsed = !self.task_pill.collapsed;
                self.task_pill.peek = false;
            }
            // The pointer on a collapsed pill, and off it.
            "peek" => self.task_pill.peek = self.task_pill.collapsed,
            "unpeek" => self.task_pill.peek = false,
            // A click on the band's Cancel.
            "cancel" => {
                if let Some(id) = self.task_pill.shown.as_ref().map(|t| t.id) {
                    self.tasks.cancel(id);
                    self.tasks_changed();
                }
            }
            _ => return "taskbox open|close|collapse|peek|unpeek|cancel".to_owned(),
        }
        self.schedule_task_pill_frame();
        self.draw_options();
        format!(
            "task pill: box {}, collapsed {}, peek {}",
            self.task_pill.box_open, self.task_pill.collapsed, self.task_pill.peek
        )
    }

    /// Where the band's chips and number are, whole (the collapsed pill's
    /// are blended in by the draw).
    fn task_band(&self, band: Rect) -> Band {
        let ph = self.options_pill_h();
        let chip_h = self.task_chip_h();
        let edge = (ph - chip_h) / 2.0;
        let chip_y = band.y + (band.h - chip_h) / 2.0;
        let right = band.x + band.w;
        let slot = self.task_pill.pct_w;
        let e = self.task_pill.box_e;
        let more = self.task_more().map(|(text, w)| (Rect::new(right - edge - w, chip_y, w, chip_h), text));
        let over = self.task_pill.shown.as_ref().is_none_or(|t| t.ended.is_some());
        // As the box opens and `+N` goes, what stood left of it slides to the
        // edge, into line with the rows under it.
        let beside = |w: f32, at_edge: f32| match &more {
            Some((chip, _)) => lerp(chip.x - CHIP_GAP - w, at_edge, e),
            None => at_edge,
        };
        if over {
            let x = beside(slot, right - PILL_PAD_X - slot);
            return Band { more, cancel: None, number: (x, slot), words_right: x - GAP };
        }
        let cw = self.task_cancel_w();
        let cancel = Rect::new(beside(cw, right - edge - cw), chip_y, cw, chip_h);
        let x = cancel.x - CHIP_GAP - slot;
        Band { more, cancel: Some(cancel), number: (x, slot), words_right: x - GAP }
    }

    /// The box's rows that the opening box has room for: each task's id and
    /// its line.
    fn task_rows(&self, rect: Rect) -> Vec<(u64, Rect)> {
        if self.task_pill.box_e < 0.01 {
            return Vec::new();
        }
        let ph = self.options_pill_h();
        let row_h = ph * ROW_H;
        self.tasks
            .rest()
            .iter()
            .enumerate()
            .map(|(idx, task)| (task.id, Rect::new(rect.x, rect.y + ph + AIR + idx as f32 * (row_h + AIR), rect.w, row_h)))
            .take_while(|(_, rr)| rr.y + rr.h <= rect.y + rect.h + 0.5)
            .collect()
    }

    /// A row's `Cancel` chip.
    fn task_row_cancel(&self, row: Rect) -> Rect {
        let chip_h = self.task_chip_h();
        let edge = (self.options_pill_h() - chip_h) / 2.0;
        let cw = self.task_cancel_w();
        Rect::new(row.x + row.w - edge - cw, row.y + (row.h - chip_h) / 2.0, cw, chip_h)
    }

    /// One amber chip, the bell's.
    fn push_task_chip(&self, scene: &mut Scene, chip: Rect, text: &str, ink: [f32; 4], alpha: f32, lit: bool) {
        if alpha <= 0.004 {
            return;
        }
        let s = self.options_scale();
        let amber = crate::notif::AMBER;
        scene.rects.push(RectInst {
            rect: chip,
            radius: chip.h / 2.0,
            // (A little stronger under the pointer: it is a button.)
            color: [amber[0], amber[1], amber[2], if lit { 0.38 } else { 0.2 } * alpha],
            glass: 0.0,
            border: 0.0,
        });
        scene.labels.push(Label {
            text: text.to_owned(),
            pos: (chip.x + chip.w / 2.0, chip.y + (chip.h - LINE_PX * s) / 2.0),
            max_w: chip.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family: TEXT_FONT,
            color: Some([ink[0], ink[1], ink[2], alpha]),
            clip: Some(chip),
        });
    }

    /// Draw it: the ground every pill has, the fill, the words, the number,
    /// the chips — and under the band, the box.
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
        // How much of the whole pill there is: collapsed, only the number
        // and `+N` are left.
        let whole = (1.0 - self.task_pill.small).clamp(0.0, 1.0);
        // The player's track fill: the pill itself, filling from the left
        // (round-ended always: the box's corner is not the bar's end).
        let fill = self.task_pill.fill.clamp(0.0, 1.0);
        if fill > 0.0 && whole > 0.01 {
            let cap = band.h / 2.0;
            let fw = (band.w * fill).max(2.0 * cap).min(band.w);
            scene.rects.push(RectInst {
                rect: Rect::new(band.x, band.y, fw, band.h),
                radius: cap,
                color: [ink[0], ink[1], ink[2], 0.14 * a * whole],
                glass: 0.0,
                border: 0.0,
            });
        }
        self.push_task_rows(scene, rect, solid, ink);

        let parts = self.task_band(band);
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Task));
        // `+N` nests in the pill's right cap, as the bell's does, and fades
        // as the box opens (the others are then in plain sight).
        if let Some((chip, text)) = &parts.more {
            self.push_task_chip(scene, *chip, text, ink, (1.0 - e) * a, false);
        }
        if let Some(cancel) = parts.cancel {
            self.push_task_chip(scene, cancel, CANCEL, ink, a * whole, ptr.is_some_and(|p| cancel.contains(p)));
        }
        let ty = band.y + (band.h - line_px) / 2.0;
        let color = Some([ink[0], ink[1], ink[2], ink[3] * a]);
        // The words, left, cut where the number's slot begins.
        let words_w = (parts.words_right - (band.x + PILL_PAD_X)).max(0.0);
        if words_w > 1.0 && whole > 0.01 {
            scene.labels.push(Label {
                text: self.task_pill_label(),
                pos: (band.x + PILL_PAD_X, ty),
                max_w: self.task_pill.label_w.max(words_w),
                font_px,
                line_px,
                centered: false,
                dim: false,
                cache: false,
                clip: Some(Rect::new(band.x + PILL_PAD_X, band.y, words_w, band.h)),
                family: TEXT_FONT,
                color: Some([ink[0], ink[1], ink[2], ink[3] * a * whole]),
            });
        }
        // The number: centred in its slot when the pill is whole, hard by
        // the left end when it is collapsed.
        let (slot_x, slot) = parts.number;
        let num_w = self.task_pill.num_w.max(1.0);
        let x = lerp(band.x + SMALL_PAD, slot_x + (slot - num_w) / 2.0, whole);
        scene.labels.push(Label {
            text: format!("{}%", task.percent()),
            pos: (x, ty),
            max_w: num_w + 4.0,
            font_px,
            line_px,
            centered: false,
            dim: false,
            cache: false,
            clip: Some(band),
            family: TEXT_FONT,
            color,
        });
    }

    /// The box's rows: the other tasks, each a line of its own like the band
    /// above them — its words, its number, its Cancel, and its fill a
    /// round-ended bar as the band's is — with `AIR` between one and the
    /// next. A row is drawn once the opening box has room for all of it.
    fn push_task_rows(&self, scene: &mut Scene, rect: Rect, solid: f32, ink: [f32; 4]) {
        let s = self.options_scale();
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);
        let slot = self.task_pill.pct_w;
        let color = Some([ink[0], ink[1], ink[2], ink[3] * solid]);
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Task));
        let rest = self.tasks.rest();
        for (id, rr) in self.task_rows(rect) {
            let Some(task) = rest.iter().find(|t| t.id == id) else {
                continue;
            };
            let radius = rr.h / 2.0;
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
            let cancel = self.task_row_cancel(rr);
            self.push_task_chip(scene, cancel, CANCEL, ink, solid, ptr.is_some_and(|p| cancel.contains(p)));
            let ty = rr.y + (rr.h - line_px) / 2.0;
            let slot_x = cancel.x - CHIP_GAP - slot;
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

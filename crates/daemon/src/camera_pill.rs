//! The CAMERA pill: on the OPTIONS bar for as long as a phone is this
//! computer's camera, and where that camera is set — Max, 2026-10-10:
//! *"lets keep the menu simple (use as camera / stop using camera) and build
//! the rest on the banner. start with one pill, i hover it, pills show up.
//! one is for toggle the flash on and off, the other one shows the current
//! quality… when i hover the quality, it expands showing the rest of the
//! qualities available. i can click on one of them to change the quality.
//! a pill to choose the lens… centered between the current task and the
//! notifications. expand to the right."*
//!
//! AT REST: one pill, `Camera`. The pointer on it brings its children out
//! from behind it, to the right (the way the gear's readout and the
//! clipboard's preview come out from behind theirs): `Flash` · the picture
//! in use · the lens in use.
//!
//! FLASH: a click turns it on or off; on, the pill is lit (the bell's amber).
//! Not there when the lens in use has none (the front one).
//!
//! PICTURE and LENS: the pointer on one grows it to the right into all there
//! is to choose from, the one in use first — where it already stood, so
//! nothing jumps — and a click on another takes it. The camera starts again
//! with what was chosen (the phone sets lens, picture and flash as its
//! camera starts): a second or two without picture.
//!
//! WHERE: its left end stands so that the resting pill is centred in the
//! ground between the window's pills and the bell, and far enough left that
//! its widest self still ends before the bell. It grows into ground no other
//! OPTION uses and pushes nothing (the Still Bar); while the pointer is on
//! it, it does not move either, whatever the window's pills do.

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::desktop::Picture;
use crate::options::{push_neumorph, PillId, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

const HEAD: &str = "Camera";
const FLASH: &str = "Flash";

/// Which child the pointer has grown.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) enum Grown {
    #[default]
    None,
    Picture,
    Lens,
}

/// The pill's state.
#[derive(Debug, Default)]
pub(crate) struct CameraPill {
    /// 0 (no camera) → 1 (there).
    t: f32,
    /// 0 (the one pill) → 1 (its children out).
    open: f32,
    /// How far the picture's pill and the lens's are grown.
    e_picture: f32,
    e_lens: f32,
    want_open: bool,
    grown: Grown,
    last: Option<Instant>,
    frame_pending: bool,
    /// What it shows, the one in use first: the pictures and the lenses
    /// (each lens with its place in the phone's list), and the flash (`None`:
    /// the lens has none).
    pictures: Vec<Picture>,
    lenses: Vec<(usize, String)>,
    torch: Option<bool>,
    /// Measured widths of the words, in the order above.
    head_w: f32,
    flash_w: f32,
    picture_w: Vec<f32>,
    lens_w: Vec<f32>,
    /// Where its left end was when the pointer came onto it (held while it
    /// is open).
    held_x: Option<f32>,
    /// Driven without a pointer (`debug-desktop "campill …"`).
    forced: Option<(bool, Grown)>,
}

/// Where everything is, at the pill's present state.
struct Parts {
    head: Rect,
    flash: Option<Rect>,
    picture: Rect,
    pictures: Vec<Rect>,
    lens: Rect,
    lenses: Vec<Rect>,
    right: f32,
}

impl App {
    /// The camera began, changed or ended: the pill follows.
    pub(crate) fn camera_pill_sync(&mut self) {
        let font_px = FONT_PX * self.options_scale();
        let (pictures, lenses, torch) = match self.camera_choices() {
            Some((on, names, offered)) => {
                let mut pictures = vec![on.picture];
                pictures.extend(offered.into_iter().filter(|p| *p != on.picture));
                let mut lenses: Vec<(usize, String)> = names.into_iter().enumerate().collect();
                if on.lens < lenses.len() {
                    let current = lenses.remove(on.lens);
                    lenses.insert(0, current);
                }
                let torch = on.torch;
                (pictures, lenses, self.camera_has_torch().then_some(torch))
            }
            None => (Vec::new(), Vec::new(), None),
        };
        if !pictures.is_empty() {
            if let Some(r) = self.options_renderer.as_mut() {
                self.camera_pill.head_w = r.measure_text(HEAD, font_px, TEXT_FONT);
                self.camera_pill.flash_w = r.measure_text(FLASH, font_px, TEXT_FONT);
                self.camera_pill.picture_w =
                    pictures.iter().map(|p| r.measure_text(&p.label(), font_px, TEXT_FONT)).collect();
                self.camera_pill.lens_w = lenses.iter().map(|(_, n)| r.measure_text(n, font_px, TEXT_FONT)).collect();
            }
            self.camera_pill.pictures = pictures;
            self.camera_pill.lenses = lenses;
            self.camera_pill.torch = torch;
        }
        self.schedule_camera_pill_frame();
        self.draw_options();
    }

    fn schedule_camera_pill_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.camera_pill.frame_pending, &mut a.camera_pill.last), App::tick_camera_pill);
    }

    /// One frame of its coming, opening, growing and going.
    fn tick_camera_pill(&mut self) {
        let now = Instant::now();
        let dt = self.camera_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.camera_pill.last = Some(now);
        let there = f32::from(u8::from(self.desktop.camera_on.is_some()));
        let settle = animation::settle_t(self.camera_pill_widest().max(1.0));
        let p = &self.camera_pill;
        let (want_open, grown) = (p.want_open, p.grown);
        let (t, a) = ease_toward(p.t, there, dt, animation::MORPH_RATE, settle);
        let (open, b) = ease_toward(p.open, f32::from(u8::from(want_open)), dt, animation::MORPH_RATE, settle);
        let want = |g: Grown| f32::from(u8::from(want_open && grown == g));
        let (e_picture, c) = ease_toward(p.e_picture, want(Grown::Picture), dt, animation::MORPH_RATE, settle);
        let (e_lens, d) = ease_toward(p.e_lens, want(Grown::Lens), dt, animation::MORPH_RATE, settle);
        let p = &mut self.camera_pill;
        (p.t, p.open, p.e_picture, p.e_lens) = (t, open, e_picture, e_lens);
        if open <= 0.01 && !want_open {
            p.held_x = None;
        }
        self.sync_options_input();
        self.draw_options();
        if a || b || c || d {
            self.schedule_camera_pill_frame();
        } else {
            self.camera_pill.last = None;
            if there == 0.0 {
                self.camera_pill.pictures.clear();
                self.camera_pill.lenses.clear();
            }
        }
    }

    /// A word's pill: the word and the pill's padding either side.
    fn camera_seg(&self, word_w: f32) -> f32 {
        (word_w + 2.0 * PILL_PAD_X).max(self.options_pill_h())
    }

    /// The pill at its widest: everything out, the wider of its two growing
    /// children grown.
    fn camera_pill_widest(&self) -> f32 {
        let p = &self.camera_pill;
        let seg = |w: &f32| self.camera_seg(*w);
        let first = |v: &Vec<f32>| v.first().map_or(0.0, seg);
        let all = |v: &Vec<f32>| v.iter().map(seg).sum::<f32>();
        let flash = if p.torch.is_some() { self.camera_seg(p.flash_w) + OPTION_GAP } else { 0.0 };
        let grown = (all(&p.picture_w) + first(&p.lens_w)).max(first(&p.picture_w) + all(&p.lens_w));
        self.camera_seg(p.head_w) + OPTION_GAP + flash + grown + OPTION_GAP
    }

    /// Where its parts are, its left end at `x`.
    fn camera_parts(&self, x: f32, y: f32, ph: f32) -> Parts {
        let p = &self.camera_pill;
        let open = p.open;
        let head = Rect::new(x, y, self.camera_seg(p.head_w), ph);
        // A child comes out from behind the head: from the head's place to
        // its own.
        let mut at = head.x + head.w + OPTION_GAP;
        let out = |to: f32| lerp(head.x, to, open);
        let flash = p.torch.map(|_| {
            let r = Rect::new(out(at), y, self.camera_seg(p.flash_w), ph);
            at += r.w + OPTION_GAP;
            r
        });
        let row = |widths: &Vec<f32>, grown: f32, at: &mut f32| {
            let mut items = Vec::new();
            let mut ix = out(*at);
            let start = ix;
            for (k, w) in widths.iter().enumerate() {
                let w = self.camera_seg(*w) * if k == 0 { 1.0 } else { grown };
                items.push(Rect::new(ix, y, w, ph));
                ix += w;
            }
            let rect = Rect::new(start, y, (ix - start).max(ph.min(1.0)), ph);
            *at += rect.w + OPTION_GAP;
            (rect, items)
        };
        let (picture, pictures) = row(&p.picture_w, p.e_picture, &mut at);
        let (lens, lenses) = row(&p.lens_w, p.e_lens, &mut at);
        let right = lerp(head.x + head.w, lens.x + lens.w, open);
        Parts { head, flash, picture, pictures, lens, lenses, right }
    }

    /// Where the pill is on the bar now (`None`: no camera): its resting
    /// self centred between the window's pills (`window_right`) and the bell
    /// (`bell_left`), its left end no further right than lets its widest
    /// self end before the bell.
    pub(crate) fn camera_pill_rect(&self, window_right: f32, bell_left: f32, y: f32, ph: f32) -> Option<Rect> {
        let p = &self.camera_pill;
        if p.t <= 0.01 || p.pictures.is_empty() {
            return None;
        }
        let x = p.held_x.unwrap_or_else(|| self.camera_pill_origin(window_right, bell_left));
        let parts = self.camera_parts(x, y, ph);
        Some(Rect::new(x, y, parts.right - x, ph))
    }

    fn camera_pill_origin(&self, window_right: f32, bell_left: f32) -> f32 {
        let head = self.camera_seg(self.camera_pill.head_w);
        let centred = (window_right + bell_left) / 2.0 - head / 2.0;
        let fits = bell_left - OPTION_GAP - self.camera_pill_widest();
        centred.min(fits).max(window_right + OPTION_GAP)
    }

    /// Its rect as the bar has it.
    fn camera_pill_drawn(&self) -> Option<Rect> {
        self.camera_pill_on_bar()
    }

    /// The pointer came onto the pill, moved on it, or left (called on every
    /// hover move over the bar). Whether the pointer is on it.
    pub(crate) fn update_camera_hover(&mut self) -> bool {
        let on = self.options_hover == Some(PillId::Camera);
        let (want_open, grown) = match self.camera_pill.forced {
            Some(forced) => forced,
            None if !on => (false, Grown::None),
            None => {
                let mut grown = self.camera_pill.grown;
                if let (Some(ptr), Some(rect)) = (self.options_ptr, self.camera_pill_drawn()) {
                    let parts = self.camera_parts(rect.x, rect.y, rect.h);
                    let over = |r: &Rect| ptr.0 >= r.x && ptr.0 < r.x + r.w;
                    if self.camera_pill.open > 0.6 {
                        if over(&parts.picture) {
                            grown = Grown::Picture;
                        } else if over(&parts.lens) {
                            grown = Grown::Lens;
                        } else if over(&parts.head) || parts.flash.as_ref().is_some_and(over) {
                            grown = Grown::None;
                        }
                    }
                }
                (true, grown)
            }
        };
        if on && self.camera_pill.held_x.is_none() {
            self.camera_pill.held_x = self.camera_pill_drawn().map(|r| r.x);
        }
        if (want_open, grown) != (self.camera_pill.want_open, self.camera_pill.grown) {
            self.camera_pill.want_open = want_open;
            self.camera_pill.grown = grown;
            self.schedule_camera_pill_frame();
        }
        on
    }

    /// The pointer left the bar: the visit is over.
    pub(crate) fn camera_pill_close(&mut self) {
        if self.camera_pill.forced.is_some() {
            return;
        }
        if self.camera_pill.want_open {
            self.camera_pill.want_open = false;
            self.camera_pill.grown = Grown::None;
            self.schedule_camera_pill_frame();
        }
    }

    /// A click on the pill: the flash, another picture, another lens.
    pub(crate) fn camera_pill_click(&mut self) {
        let (Some(ptr), Some(rect)) = (self.options_ptr, self.camera_pill_drawn()) else {
            return;
        };
        let parts = self.camera_parts(rect.x, rect.y, rect.h);
        let over = |r: &Rect| ptr.0 >= r.x && ptr.0 < r.x + r.w;
        if parts.flash.as_ref().is_some_and(over) {
            let on = self.camera_pill.torch.unwrap_or(false);
            self.camera_set(None, None, Some(!on));
        } else if let Some(k) = parts.pictures.iter().skip(1).position(over).filter(|_| self.camera_pill.e_picture > 0.5) {
            if let Some(picture) = self.camera_pill.pictures.get(k + 1).copied() {
                self.camera_set(None, Some(picture), None);
            }
        } else if let Some(k) = parts.lenses.iter().skip(1).position(over).filter(|_| self.camera_pill.e_lens > 0.5) {
            if let Some(n) = self.camera_pill.lenses.get(k + 1).map(|(n, _)| *n) {
                self.camera_set(Some(n), None, None);
            }
        }
    }

    /// The pill driven without a pointer (`debug-desktop "campill …"`):
    /// `open`, `picture`, `lens` (stood so), `close`, `free` (the pointer's
    /// again); `flash`, `p <k>`, `l <k>` — a click on that part, counted as
    /// it is drawn (0 is the one in use).
    pub(crate) fn camera_pill_debug(&mut self, what: &str) -> String {
        let mut words = what.split_whitespace();
        let k = |w: Option<&str>| w.and_then(|w| w.parse::<usize>().ok());
        match words.next() {
            Some("open") => self.camera_pill.forced = Some((true, Grown::None)),
            Some("picture") => self.camera_pill.forced = Some((true, Grown::Picture)),
            Some("lens") => self.camera_pill.forced = Some((true, Grown::Lens)),
            Some("close") => self.camera_pill.forced = Some((false, Grown::None)),
            Some("free") => self.camera_pill.forced = None,
            Some("flash") => {
                if let Some(on) = self.camera_pill.torch {
                    self.camera_set(None, None, Some(!on));
                }
            }
            Some("p") => {
                if let Some(picture) = k(words.next()).and_then(|k| self.camera_pill.pictures.get(k).copied()) {
                    self.camera_set(None, Some(picture), None);
                }
            }
            Some("l") => {
                if let Some(n) = k(words.next()).and_then(|k| self.camera_pill.lenses.get(k).map(|(n, _)| *n)) {
                    self.camera_set(Some(n), None, None);
                }
            }
            _ => return "campill open|picture|lens|close|free|flash|p <k>|l <k>".to_owned(),
        }
        self.update_camera_hover();
        self.schedule_camera_pill_frame();
        let p = &self.camera_pill;
        format!(
            "camera pill: flash {:?}, pictures {:?}, lenses {:?}, at {:?}",
            p.torch,
            p.pictures.iter().map(Picture::label).collect::<Vec<_>>(),
            p.lenses.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            self.camera_pill_drawn().map(|r| (r.x.round(), r.w.round()))
        )
    }

    /// One pill of it: the ground every pill has, the wash under the pointer
    /// a little stronger, `lit` the bell's amber.
    fn push_camera_ground(&self, scene: &mut Scene, rect: Rect, alpha: f32, hot: bool, lit: bool) {
        if alpha <= 0.004 || rect.w < 1.0 {
            return;
        }
        let radius = rect.h / 2.0;
        push_neumorph(scene, rect, radius, self.options_bar_is_bright(), alpha);
        let wash = if hot { self.options_hover_wash() } else { self.options_rest_wash() };
        scene.rects.push(RectInst {
            rect,
            radius,
            color: [wash[0], wash[1], wash[2], wash[3] * alpha],
            glass: 0.0,
            border: 0.0,
        });
        if lit {
            let amber = crate::notif::AMBER;
            scene.rects.push(RectInst {
                rect,
                radius,
                color: [amber[0], amber[1], amber[2], 0.34 * alpha],
                glass: 0.0,
                border: 0.0,
            });
        }
    }

    /// A word centred in `slot`, cut to `clip`.
    fn push_camera_word(&self, scene: &mut Scene, text: &str, slot: Rect, clip: Rect, alpha: f32) {
        if alpha <= 0.004 || slot.w < 2.0 {
            return;
        }
        let s = self.options_scale();
        let ink = self.options_text_color();
        scene.labels.push(Label {
            text: text.to_owned(),
            pos: (slot.x + slot.w / 2.0, slot.y + (slot.h - LINE_PX * s) / 2.0),
            max_w: slot.w.max(1.0) + 64.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family: TEXT_FONT,
            color: Some([ink[0], ink[1], ink[2], ink[3] * alpha]),
            clip: Some(clip),
        });
    }

    /// Draw it: the children first (they come out from behind), the head on
    /// top of them.
    pub(crate) fn push_camera_pill(&self, scene: &mut Scene, rect: Rect) {
        let p = &self.camera_pill;
        let a = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let kids = a * ((p.open - 0.1) / 0.6).clamp(0.0, 1.0);
        let parts = self.camera_parts(rect.x, rect.y, rect.h);
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Camera));
        let under = |r: &Rect| ptr.is_some_and(|p| p.0 >= r.x && p.0 < r.x + r.w);

        // A growing child: one pill; in it the one in use, then the others
        // as far as it is grown — the one under the pointer on a stronger
        // wash, the others' words lighter than the one in use.
        let row = |scene: &mut Scene, pill: Rect, items: &[Rect], words: Vec<String>, grown: f32| {
            self.push_camera_ground(scene, pill, kids, false, false);
            for (k, (item, word)) in items.iter().zip(words).enumerate() {
                if k > 0 && grown > 0.5 && under(item) {
                    let chip_h = LINE_PX * self.options_scale();
                    let edge = (item.h - chip_h) / 2.0;
                    let wash = self.options_hover_wash();
                    scene.rects.push(RectInst {
                        rect: Rect::new(item.x + edge / 2.0, item.y + edge, (item.w - edge).max(1.0), chip_h),
                        radius: chip_h / 2.0,
                        color: [wash[0], wash[1], wash[2], (wash[3] * 2.0).min(1.0) * kids * grown],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                let alpha = if k == 0 { kids } else { kids * grown * 0.72 };
                self.push_camera_word(scene, &word, *item, pill, alpha);
            }
        };
        if let Some(flash) = parts.flash {
            self.push_camera_ground(scene, flash, kids, under(&flash), p.torch == Some(true));
            self.push_camera_word(scene, FLASH, flash, flash, kids);
        }
        row(scene, parts.picture, &parts.pictures, p.pictures.iter().map(Picture::label).collect(), p.e_picture);
        row(scene, parts.lens, &parts.lenses, p.lenses.iter().map(|(_, n)| n.clone()).collect(), p.e_lens);
        self.push_camera_ground(scene, parts.head, a, under(&parts.head), false);
        self.push_camera_word(scene, HEAD, parts.head, parts.head, a);
    }
}

//! THE CAPTURE TOOL'S PILLS on the banner (Max, 2026-11-16, on seeing the
//! tool with a toolbar at the bottom of the screen: the controls as pills on
//! the banner, "a parent pill and a few children").
//!
//! WHILE THE TOOL IS UP (`capture.rs`): one round pill right of the gear —
//! where the power key's pills stand, and after them when both are there —
//! with the camera on it (a film camera while Video is lit), and its children
//! out to its right AT ONCE, as words (the tool was asked for: no hand is
//! waited for): `Region` · `Window` · `Screen`, the one in use lit; `Video`
//! (lit: a film; not lit: a picture); `Timer: off` / `Timer: 3 s` / `Timer:
//! 10 s`, a click the next; `Copy text` where text can be read (`tesseract`);
//! and, while Video is lit, `Sound`. They come out OVER what stands right of
//! the gear, and what they stand over leaves the bar for as long as they are
//! out — the bar's one rule for whatever grows over it (`options_pills`,
//! `capture_pill_span`); where the row reaches the window's pill, that pill
//! and its controls leave together. (Moving the left band over, as the power
//! pills do, pushed it into the window's pill on a 1440 px screen.)
//!
//! The idiom is the bar's own: the power pills' coming out from behind the
//! round one (`power_pill.rs`), the terminal pill's children as words, the
//! lit one in full ink and the others quieter (`term_pill.rs`); a lit one
//! also stands on the ground a pill has under the pointer. No tooltip.
//!
//! A CLICK on the round pill closes the tool (as Escape does); on a child,
//! what its word says. During a countdown only the round pill answers.
//!
//! OVER A FULLSCREEN WINDOW the banner is away: the tool calls it, as the
//! power key does, and it goes back as it came when the tool closes.
//!
//! BEFORE THE PICTURE the pills are gone AT ONCE (`capture_pills_off_now`,
//! no easing — a region or a screen can have the banner in it; a banner that
//! was called goes whole), and the picture waits for the banner's frame
//! without them as it does for the tool's own empty one.
//!
//! Rig: `waverunner-ctl capture pills` says where they stand (the log).

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp};
use crate::capture::{Ctl, Kind, Phase, Prefs, What};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::i18n::{fill, tr};
use crate::options::{
    hover_grow, push_neumorph, PillId, BOND_GAP, EDGE_PAD, FONT_PX, LINE_PX, NERD, OPTION_GAP,
    PILL_PAD_X, TEXT_FONT,
};
use crate::App;

/// fa-camera: a picture.
const GLYPH_PICTURE: &str = "\u{f030}";
/// fa-video-camera: a film.
const GLYPH_FILM: &str = "\u{f03d}";
/// A child not in use, and every child while the timer counts: this much of
/// the ink (the terminal pill's idle tab).
const REST_INK: f32 = 0.6;
const OFF_INK: f32 = 0.38;

/// The children, in order. Sound is a film's and comes last, so that
/// lighting Video moves no other pill; Copy text only where text can be read.
pub(crate) fn children(kind: Kind, ocr: bool) -> Vec<Ctl> {
    let mut all = vec![
        Ctl::What(What::Region),
        Ctl::What(What::Window),
        Ctl::What(What::Screen),
        Ctl::Video,
        Ctl::Timer,
    ];
    if ocr {
        all.push(Ctl::Text);
    }
    if kind == Kind::Video {
        all.push(Ctl::Sound);
    }
    all
}

/// A control's words (the button by the selection too: `Ctl::Main`).
pub(crate) fn words(ctl: Ctl, prefs: &Prefs) -> String {
    match ctl {
        Ctl::What(What::Region) => tr("Region").to_owned(),
        Ctl::What(What::Window) => tr("Window").to_owned(),
        Ctl::What(What::Screen) => tr("Screen").to_owned(),
        Ctl::Video => tr("Video").to_owned(),
        Ctl::Timer if prefs.timer == 0 => tr("Timer: off").to_owned(),
        Ctl::Timer => fill(tr("Timer: {n} s"), &[("n", &prefs.timer)]),
        Ctl::Sound => tr("Sound").to_owned(),
        Ctl::Text => tr("Copy text").to_owned(),
        Ctl::Main if prefs.kind == Kind::Video => tr("Record").to_owned(),
        Ctl::Main => tr("Capture").to_owned(),
    }
}

/// Whether a child is the one in use of its group, or a toggle that is on.
pub(crate) fn lit(ctl: Ctl, prefs: &Prefs) -> bool {
    match ctl {
        Ctl::What(what) => what == prefs.what,
        Ctl::Video => prefs.kind == Kind::Video,
        Ctl::Timer => prefs.timer > 0,
        Ctl::Sound => prefs.sound,
        Ctl::Text | Ctl::Main => false,
    }
}

/// The pills' places: the round one at `x`, `ph` high, then each child
/// `out` of its way from behind it to where it stands (`widths`: each
/// child's whole width).
pub(crate) fn places(x: f32, y: f32, ph: f32, widths: &[f32], out: f32) -> Vec<Rect> {
    let mut rects = vec![Rect::new(x, y, ph, ph)];
    let mut to = x + ph + BOND_GAP;
    for w in widths {
        rects.push(Rect::new(lerp(x, to, out), y, *w, ph));
        to += w + BOND_GAP;
    }
    rects
}

/// How much of the banner they take: the round pill as far as it is there
/// (`t`), the children as far as they are out.
pub(crate) fn room(ph: f32, widths: &[f32], t: f32, out: f32) -> f32 {
    let kids: f32 = widths.iter().map(|w| w + BOND_GAP).sum();
    t.clamp(0.0, 1.0) * (ph + OPTION_GAP) + out.clamp(0.0, 1.0) * kids
}

/// The pills' state.
#[derive(Debug, Default)]
pub(crate) struct CapturePills {
    /// 0 (gone) → 1 (there).
    t: f32,
    /// 0 (the children behind the round pill) → 1 (out).
    out: f32,
    /// The children as they last were (kept while the pills go, so they go
    /// with their words), and their words' widths as measured.
    kids: Vec<(Ctl, String)>,
    words_w: Vec<f32>,
    /// The banner was hidden (a fullscreen) and the tool called it.
    called_bar: bool,
    last: Option<Instant>,
    frame_pending: bool,
}

impl App {
    /// The tool changed (opened, a choice, closed): the children's words
    /// again, measured, the banner called from behind a fullscreen window,
    /// and the pills toward where they should be.
    pub(crate) fn capture_pill_sync(&mut self) {
        let on = self.capture_pills_on();
        if let Some(tool) = self.capture_tool.tool.as_ref() {
            let prefs = &self.capture_tool.prefs;
            let kids: Vec<(Ctl, String)> = children(prefs.kind, tool.ocr)
                .into_iter()
                .map(|c| (c, words(c, prefs)))
                .collect();
            if kids != self.capture_pills.kids {
                let font_px = FONT_PX * self.options_scale();
                if let Some(r) = self.options_renderer.as_ref() {
                    self.capture_pills.words_w = kids
                        .iter()
                        .map(|(_, w)| r.measure_text(w, font_px, TEXT_FONT))
                        .collect();
                }
                self.capture_pills.kids = kids;
            }
        }
        if on {
            // (Over a fullscreen the banner is away: the tool calls it.)
            if self.options_hidden {
                self.capture_pills.called_bar = true;
                self.set_options_hidden(false);
            }
        } else if self.capture_tool.tool.is_none()
            && std::mem::take(&mut self.capture_pills.called_bar)
            && self.options_fullscreen
            && self.options_ptr.is_none()
        {
            // (…and it goes back as it came when the tool does.)
            self.schedule_options_hide();
        }
        if on || self.capture_pills.t > 0.0 {
            self.schedule_capture_pill_frame();
        }
    }

    /// The pills off the banner NOW, in one frame (the picture is about to be
    /// taken and may have the banner in it); a banner the tool called over a
    /// fullscreen window goes whole, as it was before.
    pub(crate) fn capture_pills_off_now(&mut self) {
        (self.capture_pills.t, self.capture_pills.out) = (0.0, 0.0);
        if std::mem::take(&mut self.capture_pills.called_bar) && self.options_fullscreen {
            self.options_hide_now();
        }
        self.options_update_hover();
        self.sync_options_input();
        self.draw_options();
    }

    /// Where the round pill stands: right of the gear, after the power key's
    /// pills as far as they are there.
    fn capture_pill_x(&self) -> f32 {
        EDGE_PAD + self.options_pill_h() + OPTION_GAP + self.power_room()
    }

    /// Each child's whole width: its words and the pill's air at their ends.
    fn capture_pill_widths(&self) -> Vec<f32> {
        let pad = PILL_PAD_X * self.options_scale();
        let p = &self.capture_pills;
        (0..p.kids.len())
            .map(|k| 2.0 * pad + p.words_w.get(k).copied().unwrap_or(40.0))
            .collect()
    }

    /// The stretch of the banner they stand over now, with the air after
    /// them (`None`: they are not there). What stands in it leaves the bar.
    pub(crate) fn capture_pill_span(&self) -> Option<Rect> {
        let room = self.capture_pill_room();
        (room > 0.0).then(|| Rect::new(self.capture_pill_x(), 0.0, room, self.options_bar_h()))
    }

    /// How much of the banner right of the gear they take now.
    pub(crate) fn capture_pill_room(&self) -> f32 {
        let p = &self.capture_pills;
        if p.t <= 0.0 {
            return 0.0;
        }
        room(
            self.options_pill_h(),
            &self.capture_pill_widths(),
            p.t,
            p.out,
        )
    }

    /// The pills on the banner now: the round one, then its children.
    pub(crate) fn capture_pill_rects(&self, y: f32, ph: f32) -> Vec<(PillId, Rect)> {
        let p = &self.capture_pills;
        if p.t <= 0.01 {
            return Vec::new();
        }
        let ids = std::iter::once(PillId::Capture)
            .chain((0..p.kids.len()).map(|k| PillId::CaptureCtl(k as u8)));
        ids.zip(places(
            self.capture_pill_x(),
            y,
            ph,
            &self.capture_pill_widths(),
            p.out,
        ))
        .collect()
    }

    /// Whether a child is out far enough to be pointed at.
    pub(crate) fn capture_pill_reachable(&self, id: PillId) -> bool {
        id == PillId::Capture || self.capture_pills.out > 0.5
    }

    /// A click: the round pill closes the tool; a child does what it says.
    pub(crate) fn capture_pill_click(&mut self, id: PillId) {
        match id {
            PillId::CaptureCtl(k) => {
                if let Some((ctl, _)) = self.capture_pills.kids.get(usize::from(k)).cloned() {
                    self.capture_press_ctl(ctl);
                }
            }
            _ => self.capture_close(),
        }
        self.draw_options();
    }

    /// Where the pills stand, for a rig (`capture pills`).
    pub(crate) fn capture_pill_state(&self) -> String {
        let p = &self.capture_pills;
        let rects: Vec<String> = self
            .capture_pill_rects(0.0, self.options_pill_h())
            .iter()
            .map(|(id, r)| {
                let name = match id {
                    PillId::CaptureCtl(k) => p
                        .kids
                        .get(usize::from(*k))
                        .map_or_else(String::new, |(_, w)| w.clone()),
                    _ => "parent".to_owned(),
                };
                format!(
                    "[{name}] {:.0},{:.0} {:.0}x{:.0}",
                    r.x,
                    r.y + crate::options::PILL_MARGIN_Y,
                    r.w,
                    r.h
                )
            })
            .collect();
        format!(
            "pills t {:.2} out {:.2} room {:.0} called-bar {}: {}",
            p.t,
            p.out,
            self.capture_pill_room(),
            p.called_bar,
            rects.join(" ")
        )
    }

    fn schedule_capture_pill_frame(&mut self) {
        self.schedule_tick(
            |a| {
                (
                    &mut a.capture_pills.frame_pending,
                    &mut a.capture_pills.last,
                )
            },
            App::tick_capture_pill,
        );
    }

    /// One frame of their coming and going (the children come with the
    /// round pill: nothing waits for a hand).
    fn tick_capture_pill(&mut self) {
        let now = Instant::now();
        let dt = self
            .capture_pills
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.capture_pills.last = Some(now);
        let ph = self.options_pill_h();
        let span: f32 = self
            .capture_pill_widths()
            .iter()
            .map(|w| w + BOND_GAP)
            .sum();
        let on = f32::from(u8::from(self.capture_pills_on()));
        let p = &mut self.capture_pills;
        let (t, a) = ease_toward(
            p.t,
            on,
            dt,
            animation::MORPH_RATE,
            animation::settle_t(ph + OPTION_GAP),
        );
        let (out, b) = ease_toward(
            p.out,
            on,
            dt,
            animation::MORPH_RATE,
            animation::settle_t(span.max(1.0)),
        );
        (p.t, p.out) = (t, out);
        self.sync_options_input();
        self.draw_options();
        if a || b {
            self.schedule_capture_pill_frame();
        } else {
            self.capture_pills.last = None;
        }
    }

    /// One of the pills drawn: the round one with its camera, or a child's
    /// word.
    pub(crate) fn push_capture_pill(&self, scene: &mut Scene, rect: Rect, id: PillId) {
        let p = &self.capture_pills;
        let prefs = &self.capture_tool.prefs;
        let shown = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let a = if id == PillId::Capture {
            shown
        } else {
            shown * ((p.out - 0.15) / 0.6).clamp(0.0, 1.0)
        };
        if a <= 0.004 {
            return;
        }
        let kid = match id {
            PillId::CaptureCtl(k) => p.kids.get(usize::from(k)),
            _ => None,
        };
        // (During a countdown only the round pill answers.)
        let counting = self
            .capture_tool
            .tool
            .as_ref()
            .is_some_and(|t| matches!(t.phase, Phase::Counting { .. }));
        let on = kid.is_some_and(|(ctl, _)| lit(*ctl, prefs));
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(id) && !(counting && kid.is_some());
        let r = if hovered { hover_grow(rect) } else { rect };
        push_neumorph(scene, r, r.h / 2.0, bright, a);
        // (The one in use stands on the ground a pill has under the pointer.)
        let mut wash = if hovered || on {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
        wash[3] *= a;
        scene.rects.push(RectInst {
            rect: r,
            radius: r.h / 2.0,
            color: wash,
            glass: 0.0,
            border: 0.0,
        });
        // (Labels are drawn in one late pass, over every pill: a child coming
        // out keeps its word off the pill it comes from behind.)
        let rects = self.capture_pill_rects(rect.y, rect.h);
        let before = rects
            .iter()
            .position(|(i, _)| *i == id)
            .and_then(|k| k.checked_sub(1))
            .map(|k| rects[k].1.x + rects[k].1.w);
        let clip = match before {
            Some(edge) if edge > r.x => Rect::new(edge, r.y, (r.x + r.w - edge).max(0.0), r.h),
            _ => r,
        };
        let s = self.options_scale();
        let ink = self.options_text_color();
        let (text, family, k) = match kid {
            Some((_, word)) => (
                word.clone(),
                TEXT_FONT,
                if counting {
                    OFF_INK
                } else if on || hovered {
                    1.0
                } else {
                    REST_INK
                },
            ),
            None => (
                (if prefs.kind == Kind::Video {
                    GLYPH_FILM
                } else {
                    GLYPH_PICTURE
                })
                .to_owned(),
                Some(NERD),
                1.0,
            ),
        };
        scene.labels.push(Label {
            text,
            pos: (r.x + r.w / 2.0, r.y + (r.h - LINE_PX * s) / 2.0),
            max_w: r.w + 4.0,
            font_px: FONT_PX * s,
            line_px: LINE_PX * s,
            centered: true,
            dim: false,
            cache: true,
            family,
            color: Some([ink[0], ink[1], ink[2], ink[3] * k * a]),
            clip: Some(clip),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_is_a_films_and_last_and_copy_text_only_where_text_can_be_read() {
        assert_eq!(
            children(Kind::Picture, false),
            [
                Ctl::What(What::Region),
                Ctl::What(What::Window),
                Ctl::What(What::Screen),
                Ctl::Video,
                Ctl::Timer
            ]
        );
        assert_eq!(children(Kind::Picture, true).last(), Some(&Ctl::Text));
        // Lighting Video adds Sound AFTER everything: no other pill moves.
        let (picture, film) = (children(Kind::Picture, true), children(Kind::Video, true));
        assert_eq!(film[..picture.len()], picture[..]);
        assert_eq!(film.last(), Some(&Ctl::Sound));
        // (No pointer control: pictures without it, films with it.)
        assert_eq!(film.len(), 7);
    }

    #[test]
    fn a_child_says_what_it_is_set_to_and_is_lit_when_in_use() {
        let mut prefs = Prefs::default();
        assert_eq!(words(Ctl::Timer, &prefs), "Timer: off");
        assert_eq!(words(Ctl::Main, &prefs), "Capture");
        assert!(lit(Ctl::What(What::Region), &prefs) && !lit(Ctl::What(What::Screen), &prefs));
        assert!(
            !lit(Ctl::Video, &prefs)
                && !lit(Ctl::Timer, &prefs)
                && !lit(Ctl::Sound, &prefs)
                && !lit(Ctl::Text, &prefs)
        );
        prefs.timer = 10;
        prefs.kind = Kind::Video;
        prefs.sound = true;
        prefs.what = What::Window;
        assert_eq!(words(Ctl::Timer, &prefs), "Timer: 10 s");
        assert_eq!(words(Ctl::Main, &prefs), "Record");
        assert!(lit(Ctl::Video, &prefs) && lit(Ctl::Timer, &prefs) && lit(Ctl::Sound, &prefs));
        assert!(lit(Ctl::What(What::Window), &prefs) && !lit(Ctl::What(What::Region), &prefs));
    }

    #[test]
    fn the_children_come_out_from_behind_the_round_pill_to_its_right() {
        let widths = [60.0, 70.0, 50.0];
        // Behind it: every child starts at the round pill's own place.
        let behind = places(40.0, 3.0, 22.0, &widths, 0.0);
        assert_eq!(behind.len(), 4);
        assert!(behind
            .iter()
            .all(|r| r.x == 40.0 && r.y == 3.0 && r.h == 22.0));
        // Out: in a row, a bond's gap between two, none on another.
        let out = places(40.0, 3.0, 22.0, &widths, 1.0);
        assert_eq!(out[0], Rect::new(40.0, 3.0, 22.0, 22.0));
        let mut right = out[0].x + out[0].w;
        for (r, w) in out[1..].iter().zip(widths) {
            assert_eq!(r.x, right + BOND_GAP);
            assert_eq!(r.w, w);
            right = r.x + r.w;
        }
        // What stands right of them begins a gap after the last one.
        assert_eq!(40.0 + room(22.0, &widths, 1.0, 1.0), right + OPTION_GAP);
        // Half-way they take half the children's room, and gone, none.
        assert_eq!(room(22.0, &widths, 0.0, 0.0), 0.0);
        assert!(room(22.0, &widths, 1.0, 0.5) < room(22.0, &widths, 1.0, 1.0));
    }
}

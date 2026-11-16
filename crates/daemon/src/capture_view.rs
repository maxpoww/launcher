//! THE CAPTURE TOOL as it is drawn (`capture.rs` is the tool; Max,
//! 2026-11-16): the light dim outside what is chosen, its outline, a
//! region's eight handles and its size, the countdown — and the TOOLBAR,
//! bottom centre: one slim panel in the ONE MENU's paint
//! (`App::desktop_panel_paint`), its controls a menu's rows laid side by
//! side — words at the menu's size, the one in use on the menu's quiet
//! accent tint, a hairline rule between groups, the main action on THE KIT's
//! main tint, × last. No symbol of its own, no filled button, no tooltip.
//!
//! `slots` says which controls there are, `lay` places them (both pure);
//! `App::capture_layout` measures the words and keeps the rectangles for the
//! pointer, `App::capture_push` draws. A bar wider than its screen is made
//! smaller, words and air together (`fit`).

use crate::capture::{handles, Area, Ctl, Kind, Phase, Prefs, What};
use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_MEDIUM, UNLIT};
use crate::i18n::{fill, tr};
use crate::App;

/// The bar and a control in it, the menu's row high.
const BAR_H: f32 = 40.0;
const CTL_H: f32 = 28.0;
/// The air at a control's ends, between two, round a rule, inside the bar.
const PAD_X: f32 = 11.0;
const GAP: f32 = 2.0;
const RULE: f32 = 13.0;
const EDGE: f32 = 6.0;
/// How far over the screen's bottom edge the bar stands (clear of a dock).
const BOTTOM: f32 = 84.0;
/// The menu's words.
const FONT_PX: f32 = 12.5;
const LINE_PX: f32 = 16.0;
/// The least a bar is made smaller to fit a narrow screen.
const K_MIN: f32 = 0.7;
/// The dim outside what is chosen.
const DIM: f32 = 0.3;
/// Ink of what is not lit, of what cannot be done now, of a rule.
const REST_INK: f32 = 0.72;
const OFF_INK: f32 = 0.38;
const LINE_INK: f32 = 0.12;

/// The bar's controls in order; `None` is a rule between two groups. Sound
/// is a film's; Copy text only where text can be read.
pub(crate) fn slots(kind: Kind, ocr: bool) -> Vec<Option<Ctl>> {
    let mut all = vec![
        Some(Ctl::What(What::Region)),
        Some(Ctl::What(What::Window)),
        Some(Ctl::What(What::Screen)),
        None,
        Some(Ctl::Kind(Kind::Picture)),
        Some(Ctl::Kind(Kind::Video)),
        None,
        Some(Ctl::Timer),
        Some(Ctl::Pointer),
    ];
    if kind == Kind::Video {
        all.push(Some(Ctl::Sound));
    }
    all.push(None);
    if ocr {
        all.push(Some(Ctl::Text));
    }
    all.extend([Some(Ctl::Main), Some(Ctl::Close)]);
    all
}

/// A control's words.
pub(crate) fn words(ctl: Ctl, prefs: &Prefs) -> String {
    match ctl {
        Ctl::What(What::Region) => tr("Region").to_owned(),
        Ctl::What(What::Window) => tr("Window").to_owned(),
        Ctl::What(What::Screen) => tr("Screen").to_owned(),
        Ctl::Kind(Kind::Picture) => tr("Picture").to_owned(),
        Ctl::Kind(Kind::Video) => tr("Video").to_owned(),
        Ctl::Timer if prefs.timer == 0 => tr("Timer: off").to_owned(),
        Ctl::Timer => fill(tr("Timer: {n} s"), &[("n", &prefs.timer)]),
        Ctl::Pointer if prefs.pointer => tr("Pointer: shown").to_owned(),
        Ctl::Pointer => tr("Pointer: hidden").to_owned(),
        Ctl::Sound if prefs.sound => tr("Sound: on").to_owned(),
        Ctl::Sound => tr("Sound: off").to_owned(),
        Ctl::Text => tr("Copy text").to_owned(),
        Ctl::Main if prefs.kind == Kind::Video => tr("Record").to_owned(),
        Ctl::Main => tr("Capture").to_owned(),
        Ctl::Close => "×".to_owned(),
    }
}

/// Whether a control is the one in use of its group, or a chip that is on.
fn lit(ctl: Ctl, prefs: &Prefs) -> bool {
    match ctl {
        Ctl::What(what) => what == prefs.what,
        Ctl::Kind(kind) => kind == prefs.kind,
        Ctl::Timer => prefs.timer > 0,
        Ctl::Pointer => prefs.pointer,
        Ctl::Sound => prefs.sound,
        Ctl::Text | Ctl::Main | Ctl::Close => false,
    }
}

/// A control's width for words `w` wide, at size `k`.
fn ctl_w(ctl: Ctl, w: f32, k: f32) -> f32 {
    match ctl {
        Ctl::Close => CTL_H * k,
        _ => (w + 2.0 * PAD_X * k).ceil(),
    }
}

/// How wide the bar is for these controls (each with its words' width).
pub(crate) fn bar_w(sized: &[(Option<Ctl>, f32)], k: f32) -> f32 {
    let inside: f32 = sized
        .iter()
        .map(|(slot, w)| slot.map_or(RULE * k, |c| ctl_w(c, *w, k)))
        .sum();
    2.0 * EDGE * k + inside + GAP * k * sized.len().saturating_sub(1) as f32
}

/// How much smaller a bar `wide` at full size is made for a screen `room`
/// wide: not at all while it fits with a little air, never under `K_MIN`.
pub(crate) fn fit(wide: f32, room: f32) -> f32 {
    ((room - 24.0) / wide.max(1.0)).clamp(K_MIN, 1.0)
}

/// The bar at the bottom centre of a screen `room` big: its panel, each
/// control's place, and where the rules stand (their x).
pub(crate) fn lay(
    sized: &[(Option<Ctl>, f32)],
    room: (f32, f32),
    k: f32,
) -> (Rect, Vec<(Ctl, Rect)>, Vec<f32>) {
    let (w, h) = (bar_w(sized, k), BAR_H * k);
    let x0 = ((room.0 - w) / 2.0).round().max(0.0);
    let y0 = (room.1 - BOTTOM - h).round().max(0.0);
    let (mut ctls, mut rules) = (Vec::new(), Vec::new());
    let mut x = x0 + EDGE * k;
    for (slot, words_w) in sized {
        match slot {
            Some(ctl) => {
                let cw = ctl_w(*ctl, *words_w, k);
                ctls.push((
                    *ctl,
                    Rect::new(x, y0 + (h - CTL_H * k) / 2.0, cw, CTL_H * k),
                ));
                x += cw + GAP * k;
            }
            None => {
                rules.push((x + RULE * k / 2.0).round());
                x += (RULE + GAP) * k;
            }
        }
    }
    (Rect::new(x0, y0, w, h), ctls, rules)
}

fn solid(rect: Rect, radius: f32, color: [f32; 4]) -> RectInst {
    // (`UNLIT`: the dark look's night light leaves the tool's own marks alone.)
    RectInst {
        rect,
        radius,
        color,
        glass: UNLIT,
        border: 0.0,
    }
}

fn line(rect: Rect, radius: f32, color: [f32; 4], border: f32) -> RectInst {
    RectInst {
        rect,
        radius,
        color,
        glass: UNLIT,
        border,
    }
}

/// Words centred on `cx`, their line's top at `y`.
fn centred(
    text: &str,
    cx: f32,
    y: f32,
    px: f32,
    color: [f32; 4],
    family: Option<&'static str>,
    clip: Rect,
) -> Label {
    Label {
        text: text.to_owned(),
        pos: (cx, y),
        max_w: clip.w,
        font_px: px,
        line_px: px * 1.28,
        centered: true,
        dim: false,
        cache: true,
        clip: Some(clip),
        family,
        color: Some(color),
    }
}

impl App {
    /// Lay the bar out for what is chosen now (its words measured as they
    /// are drawn) and keep its rectangles for the pointer.
    pub(crate) fn capture_layout(&mut self) {
        let prefs = &self.capture_tool.prefs;
        let (Some(tool), Some(renderer)) = (
            self.capture_tool.tool.as_mut(),
            self.desktop_top.renderer.as_ref(),
        ) else {
            return;
        };
        let room = (tool.screen.w as f32, tool.screen.h as f32);
        let family = |ctl: Ctl| (ctl == Ctl::Main).then_some(FONT_MEDIUM);
        let measured = |k: f32| -> Vec<(Option<Ctl>, f32)> {
            slots(prefs.kind, tool.ocr)
                .into_iter()
                .map(|slot| {
                    (
                        slot,
                        slot.map_or(0.0, |c| {
                            renderer.measure_text(&words(c, prefs), FONT_PX * k, family(c))
                        }),
                    )
                })
                .collect()
        };
        let full = measured(1.0);
        let k = fit(bar_w(&full, 1.0), room.0);
        let sized = if k < 1.0 { measured(k) } else { full };
        let (bar_rect, bar, rules) = lay(&sized, room, k);
        (tool.bar_rect, tool.bar, tool.rules, tool.k) = (bar_rect, bar, rules, k);
    }

    /// The tool, into `scene` (nothing while it waits to be off the screen).
    pub(crate) fn capture_push(&self, scene: &mut Scene) {
        let prefs = &self.capture_tool.prefs;
        let Some(tool) = self
            .capture_tool
            .tool
            .as_ref()
            .filter(|t| !matches!(t.phase, Phase::Hiding { .. }))
        else {
            return;
        };
        let (sw, sh) = (tool.screen.w as f32, tool.screen.h as f32);
        let full = Rect::new(0.0, 0.0, sw, sh);
        let choosing = tool.phase == Phase::Choosing;
        let chosen = tool.selection(prefs.what);
        let white = |a: f32| [1.0, 1.0, 1.0, a];
        let black = |a: f32| [0.0, 0.0, 0.0, a];
        let measure = |text: &str, px: f32| {
            self.desktop_top
                .renderer
                .as_ref()
                .map_or(text.chars().count() as f32 * px * 0.56, |r| {
                    r.measure_text(text, px, None)
                })
        };
        let mut over = GridContent {
            clip: full,
            ..Default::default()
        };
        // A plate of the tool's own on the screen: dark, its words white
        // (they stand on anything).
        let plate = |over: &mut GridContent, text: &str, cx: f32, y: f32, px: f32| {
            let w = measure(text, px) + 18.0;
            let h = px * 1.28 + 8.0;
            let x = (cx - w / 2.0).clamp(4.0, (sw - w - 4.0).max(4.0));
            over.rects
                .push(solid(Rect::new(x, y, w, h), h / 2.0, black(0.62)));
            over.labels.push(centred(
                text,
                x + w / 2.0,
                y + 4.0,
                px,
                white(0.96),
                None,
                full,
            ));
        };

        // THE DIM, lightly, outside what is chosen (a whole screen chosen:
        // none of it).
        match (prefs.what, chosen) {
            (What::Screen, _) => {}
            (_, Some(a)) => {
                let r = a.rect();
                for band in [
                    Rect::new(0.0, 0.0, sw, r.y),
                    Rect::new(0.0, r.y + r.h, sw, sh - r.y - r.h),
                    Rect::new(0.0, r.y, r.x, r.h),
                    Rect::new(r.x + r.w, r.y, sw - r.x - r.w, r.h),
                ] {
                    if band.w > 0.0 && band.h > 0.0 {
                        over.rects.push(solid(band, 0.0, black(DIM)));
                    }
                }
            }
            (_, None) => over.rects.push(solid(full, 0.0, black(DIM))),
        }

        // ITS OUTLINE: a white hairline with a dark one round it (it reads
        // on a white page and on a black one).
        if let Some(a) = chosen {
            let r = a.rect();
            let (inner, outer) = if prefs.what == What::Screen {
                (Rect::new(1.0, 1.0, sw - 2.0, sh - 2.0), full)
            } else {
                (
                    Rect::new(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0),
                    Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0),
                )
            };
            over.rects.push(line(outer, 0.0, black(0.45), 1.0));
            over.rects.push(line(inner, 0.0, white(0.95), 1.0));
        }

        if let (What::Region | What::Window, Some(a)) = (prefs.what, chosen) {
            let r = a.rect();
            // A region's eight handles, while it can be changed.
            if prefs.what == What::Region && choosing {
                for (_, (x, y)) in handles(a) {
                    over.rects.push(solid(
                        Rect::new(x - 5.0, y - 5.0, 10.0, 10.0),
                        5.0,
                        black(0.5),
                    ));
                    over.rects.push(solid(
                        Rect::new(x - 4.0, y - 4.0, 8.0, 8.0),
                        4.0,
                        white(1.0),
                    ));
                }
            }
            // Its size, by it: under its left corner, inside where there is
            // no room under.
            let size = format!("{} × {}", a.w, a.h);
            let px = 11.5;
            let h = px * 1.28 + 8.0;
            let w = measure(&size, px) + 18.0;
            let y = if r.y + r.h + 8.0 + h <= sh {
                r.y + r.h + 8.0
            } else {
                (r.y + r.h - h - 8.0).max(0.0)
            };
            plate(&mut over, &size, r.x + w / 2.0, y, px);
        } else if chosen.is_none() && choosing {
            // Nothing chosen yet: what to do, said once, in the middle.
            let hint = if prefs.what == What::Region {
                tr("Drag to choose a region")
            } else {
                tr("There is no window on this screen")
            };
            plate(&mut over, hint, sw / 2.0, sh / 2.0 - 14.0, 14.0);
        }

        // THE COUNTDOWN, in the middle of what is chosen.
        if let Phase::Counting { left, .. } = tool.phase {
            let r = chosen.map_or(full, Area::rect);
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let side = 124.0;
            over.rects.push(solid(
                Rect::new(cx - side / 2.0, cy - side / 2.0, side, side),
                side / 2.0,
                black(0.5),
            ));
            let px = 64.0;
            over.labels.push(centred(
                &left.to_string(),
                cx,
                cy - px * 1.28 / 2.0,
                px,
                white(0.97),
                Some(FONT_MEDIUM),
                full,
            ));
        }
        scene.grids.push(over);

        // THE TOOLBAR.
        let (bar, k) = (tool.bar_rect, tool.k);
        if bar.w <= 0.0 {
            return;
        }
        let paint = self.desktop_panel_paint(bar);
        let dark = crate::kit::ink_is_light(paint.ink);
        let ink = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a];
        let mut g = GridContent {
            clip: bar,
            ..Default::default()
        };
        let radius = (paint.radius * k).min(bar.h / 2.0);
        g.rects.push(RectInst {
            rect: bar,
            radius,
            color: paint.fill,
            glass: paint.glass,
            border: 0.0,
        });
        g.rects.push(line(bar, radius, ink(LINE_INK), 1.0));
        for x in &tool.rules {
            g.rects.push(solid(
                Rect::new(
                    *x,
                    (bar.y + (bar.h - 16.0 * k) / 2.0).round(),
                    1.0,
                    (16.0 * k).round(),
                ),
                0.0,
                ink(LINE_INK),
            ));
        }
        for (ctl, r) in &tool.bar {
            let (ctl, r) = (*ctl, *r);
            let can = match ctl {
                Ctl::Close => true,
                _ if !choosing => false,
                Ctl::Main | Ctl::Text => chosen.is_some(),
                _ => true,
            };
            let pointed = can && tool.hover == Some(ctl);
            let on = lit(ctl, prefs);
            let row = crate::kit::ROW_RADIUS * k;
            // The ground: the menu's tint under the one in use, THE KIT's
            // main tint under the main action, a breath under the pointer.
            if ctl == Ctl::Main {
                let tint = crate::kit::main_tint(dark, pointed);
                let c = crate::kit::linear(tint);
                g.rects.push(solid(
                    r,
                    row,
                    [
                        c[0],
                        c[1],
                        c[2],
                        tint[3] * paint.ink[3] * if can { 1.0 } else { 0.5 },
                    ],
                ));
            } else if on {
                let mut c = paint.row_tint();
                c[3] *= if can { 1.0 } else { 0.6 };
                g.rects.push(solid(r, row, c));
            } else if pointed {
                g.rects.push(solid(r, row, ink(0.08)));
            }
            let color = match ctl {
                Ctl::Main => {
                    let c = crate::kit::linear(crate::kit::accent_on(dark));
                    [c[0], c[1], c[2], if can { 1.0 } else { 0.45 }]
                }
                _ if !can => ink(OFF_INK),
                _ if on || pointed => ink(1.0),
                _ => ink(REST_INK),
            };
            let (px, family) = match ctl {
                Ctl::Close => (17.0 * k, None),
                Ctl::Main => (FONT_PX * k, Some(FONT_MEDIUM)),
                _ => (FONT_PX * k, None),
            };
            let line_px = if ctl == Ctl::Close {
                px * 1.28
            } else {
                LINE_PX * k
            };
            g.labels.push(Label {
                line_px,
                ..centred(
                    &words(ctl, prefs),
                    r.x + r.w / 2.0,
                    r.y + (r.h - line_px) / 2.0,
                    px,
                    color,
                    family,
                    bar,
                )
            });
        }
        scene.grids.push(g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(kind: Kind, ocr: bool) -> Vec<(Option<Ctl>, f32)> {
        slots(kind, ocr)
            .into_iter()
            .map(|s| (s, s.map_or(0.0, |_| 50.0)))
            .collect()
    }

    #[test]
    fn sound_is_a_films_and_copy_text_only_where_text_can_be_read() {
        let has = |kind, ocr, ctl| slots(kind, ocr).contains(&Some(ctl));
        assert!(!has(Kind::Picture, false, Ctl::Sound) && has(Kind::Video, false, Ctl::Sound));
        assert!(!has(Kind::Picture, false, Ctl::Text) && has(Kind::Picture, true, Ctl::Text));
        // The main action and × end it, whatever else is there.
        for (kind, ocr) in [(Kind::Picture, false), (Kind::Video, true)] {
            let all = slots(kind, ocr);
            assert_eq!(&all[all.len() - 2..], [Some(Ctl::Main), Some(Ctl::Close)]);
            assert_eq!(
                all.iter()
                    .filter(|s| matches!(s, Some(Ctl::What(_))))
                    .count(),
                3
            );
        }
    }

    #[test]
    fn the_bar_stands_at_the_bottom_centre_with_its_controls_in_a_row_inside_it() {
        let room = (1440.0, 900.0);
        let (bar, ctls, rules) = lay(&sized(Kind::Video, true), room, 1.0);
        assert!(
            (bar.x + bar.w / 2.0 - 720.0).abs() <= 1.0,
            "centred: {bar:?}"
        );
        assert_eq!(bar.y + bar.h, 900.0 - BOTTOM);
        assert_eq!(ctls.len(), 11);
        assert_eq!(rules.len(), 3);
        let mut right = bar.x;
        for (ctl, r) in &ctls {
            assert!(
                r.x >= right && r.x + r.w <= bar.x + bar.w + 0.01,
                "{ctl:?} {r:?} in {bar:?}"
            );
            assert!(r.y >= bar.y && r.y + r.h <= bar.y + bar.h);
            right = r.x + r.w;
        }
        // Each rule stands between two controls, on none.
        for x in rules {
            assert!(ctls.iter().all(|(_, r)| x < r.x || x > r.x + r.w));
        }
        assert_eq!(bar_w(&sized(Kind::Video, true), 1.0), bar.w);
    }

    #[test]
    fn a_bar_wider_than_its_screen_is_made_smaller_but_not_past_reading() {
        assert_eq!(fit(800.0, 1440.0), 1.0);
        let k = fit(1000.0, 900.0);
        assert!(k < 1.0 && k > K_MIN, "{k}");
        assert_eq!(fit(4000.0, 800.0), K_MIN);
    }

    #[test]
    fn a_chip_says_what_it_is_set_to() {
        let mut prefs = Prefs::default();
        assert_eq!(words(Ctl::Timer, &prefs), "Timer: off");
        assert_eq!(words(Ctl::Main, &prefs), "Capture");
        assert!(
            !lit(Ctl::Timer, &prefs)
                && lit(Ctl::What(What::Region), &prefs)
                && !lit(Ctl::What(What::Screen), &prefs)
        );
        prefs.timer = 10;
        prefs.kind = Kind::Video;
        prefs.pointer = true;
        assert_eq!(words(Ctl::Timer, &prefs), "Timer: 10 s");
        assert_eq!(words(Ctl::Main, &prefs), "Record");
        assert_eq!(words(Ctl::Pointer, &prefs), "Pointer: shown");
        assert!(
            lit(Ctl::Timer, &prefs)
                && lit(Ctl::Pointer, &prefs)
                && lit(Ctl::Kind(Kind::Video), &prefs)
        );
    }
}

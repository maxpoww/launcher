//! THE CAPTURE TOOL as it is drawn on the screen (`capture.rs` is the tool,
//! `capture_pill.rs` its controls on the banner; Max, 2026-11-16): the light
//! dim outside what is chosen (never over the banner's strip, where its
//! pills stand), its outline, a region's eight handles and its size, the
//! countdown — and ONE small button by the selection, `Capture` or `Record`:
//! the visible way to take it besides Enter and the double click. It stands
//! just under the selection's bottom right corner, inside where there is no
//! room below, in the ONE MENU's paint with THE KIT's main tint and the
//! accent's words.
//!
//! `button_place` says where the button stands (pure); `App::capture_layout`
//! measures its words and keeps its rectangle for the pointer,
//! `App::capture_push` draws.

use crate::capture::{handles, Area, Ctl, Phase, What};
use crate::capture_pill::words;
use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_MEDIUM, UNLIT};
use crate::i18n::tr;
use crate::App;

/// The button: a menu's row high, its words the menu's, the air at their ends.
const BUTTON_H: f32 = 28.0;
const BUTTON_PAD: f32 = 14.0;
const FONT_PX: f32 = 12.5;
const LINE_PX: f32 = 16.0;
/// Between the selection and what stands by it (its size, the button).
const BY: f32 = 8.0;
/// The size plate's words.
const SIZE_PX: f32 = 11.5;
/// The dim outside what is chosen.
const DIM: f32 = 0.3;
const LINE_INK: f32 = 0.12;

/// A plate's height for words `px` big (the size's, a hint's).
fn plate_h(px: f32) -> f32 {
    px * 1.28 + 8.0
}

/// Where the button stands for selection `sel` on a screen `room` big: just
/// under its bottom right corner, their right edges in line; inside, over
/// that corner, where there is no room below. `left` is where the size's
/// plate ends (it stands under the left corner): on a narrow selection the
/// button begins after it. Never off the screen.
pub(crate) fn button_place(sel: Rect, w: f32, room: (f32, f32), left: f32) -> Rect {
    let below = sel.y + sel.h + BY + BUTTON_H <= room.1;
    let y = if below {
        sel.y + sel.h + BY
    } else {
        (sel.y + sel.h - BY - BUTTON_H).max(0.0)
    };
    let x = (sel.x + sel.w - w).max(left + BY);
    // (Inside, the selection's own edge is kept clear too.)
    let x = if below {
        x
    } else {
        x.min(sel.x + sel.w - BY - w).max(left + BY)
    };
    Rect::new(
        x.clamp(0.0, (room.0 - w).max(0.0)).round(),
        y.round(),
        w,
        BUTTON_H,
    )
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
    /// How wide `text` is at `px` on the tool's surface.
    fn capture_measure(&self, text: &str, px: f32, family: Option<&str>) -> f32 {
        self.desktop_top
            .renderer
            .as_ref()
            .map_or(text.chars().count() as f32 * px * 0.56, |r| {
                r.measure_text(text, px, family)
            })
    }

    /// Where the size's plate stands by selection `a`: under its left
    /// corner, inside where there is no room under.
    fn capture_size_plate(&self, a: Area, sh: f32) -> (String, Rect) {
        let r = a.rect();
        let size = format!("{} × {}", a.w, a.h);
        let (w, h) = (
            self.capture_measure(&size, SIZE_PX, None) + 18.0,
            plate_h(SIZE_PX),
        );
        let y = if r.y + r.h + BY + h <= sh {
            r.y + r.h + BY
        } else {
            (r.y + r.h - h - BY).max(0.0)
        };
        (size, Rect::new(r.x.max(4.0), y, w, h))
    }

    /// Place the button by what is chosen now (its words measured as they
    /// are drawn) and keep its rectangle for the pointer; none while nothing
    /// is chosen or the timer counts.
    pub(crate) fn capture_layout(&mut self) {
        let prefs = &self.capture_tool.prefs;
        let Some(tool) = self.capture_tool.tool.as_ref() else {
            return;
        };
        let room = (tool.screen.w as f32, tool.screen.h as f32);
        let place = match (tool.selection(prefs.what), &tool.phase) {
            (Some(a), Phase::Choosing) => {
                let w = self
                    .capture_measure(&words(Ctl::Main, prefs), FONT_PX, Some(FONT_MEDIUM))
                    .ceil()
                    + 2.0 * BUTTON_PAD;
                // (A whole screen has no size plate.)
                let left = if prefs.what == What::Screen {
                    -BY
                } else {
                    self.capture_size_plate(a, room.1).1.x + self.capture_size_plate(a, room.1).1.w
                };
                // (Only what is under the selection's left corner is in its way.)
                let left = if left + BY + w > a.rect().x + a.rect().w {
                    left
                } else {
                    -BY
                };
                button_place(a.rect(), w, room, left)
            }
            _ => Rect::new(0.0, 0.0, 0.0, 0.0),
        };
        if let Some(tool) = self.capture_tool.tool.as_mut() {
            tool.bar_rect = place;
        }
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
        let mut over = GridContent {
            clip: full,
            ..Default::default()
        };
        // A plate of the tool's own on the screen: dark, its words white
        // (they stand on anything).
        let plate = |over: &mut GridContent, text: &str, r: Rect, px: f32| {
            over.rects.push(solid(r, r.h / 2.0, black(0.62)));
            over.labels.push(centred(
                text,
                r.x + r.w / 2.0,
                r.y + 4.0,
                px,
                white(0.96),
                None,
                full,
            ));
        };

        // THE DIM, lightly, outside what is chosen (a whole screen chosen:
        // none of it) — and never on the banner's strip: its pills are the
        // tool's controls.
        let strip = tool.strip as f32;
        let under = Rect::new(0.0, strip, sw, (sh - strip).max(0.0));
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
                    let band = band.intersect(&under);
                    if band.w > 0.0 && band.h > 0.0 {
                        over.rects.push(solid(band, 0.0, black(DIM)));
                    }
                }
            }
            (_, None) => over.rects.push(solid(under, 0.0, black(DIM))),
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
            // Its size, by it.
            let (size, at) = self.capture_size_plate(a, sh);
            plate(&mut over, &size, at, SIZE_PX);
        } else if chosen.is_none() && choosing {
            // Nothing chosen yet: what to do, said once, in the middle.
            let hint = if prefs.what == What::Region {
                tr("Drag to choose a region")
            } else {
                tr("There is no window on this screen")
            };
            let (w, h) = (self.capture_measure(hint, 14.0, None) + 18.0, plate_h(14.0));
            plate(
                &mut over,
                hint,
                Rect::new((sw - w) / 2.0, sh / 2.0 - 14.0, w, h),
                14.0,
            );
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

        // THE BUTTON by the selection: the menu's panel, the main action's
        // tint on it (a step stronger under the pointer), the accent's words.
        let b = tool.bar_rect;
        if b.w <= 0.0 {
            return;
        }
        let paint = self.desktop_panel_paint(b);
        let dark = crate::kit::ink_is_light(paint.ink);
        let pointed = tool.hover == Some(Ctl::Main);
        let mut g = GridContent {
            clip: b,
            ..Default::default()
        };
        let radius = crate::kit::ROW_RADIUS;
        g.rects.push(RectInst {
            rect: b,
            radius,
            color: paint.fill,
            glass: paint.glass,
            border: 0.0,
        });
        let tint = crate::kit::main_tint(dark, pointed);
        let c = crate::kit::linear(tint);
        g.rects
            .push(solid(b, radius, [c[0], c[1], c[2], tint[3] * paint.ink[3]]));
        g.rects.push(line(
            b,
            radius,
            [
                paint.ink[0],
                paint.ink[1],
                paint.ink[2],
                paint.ink[3] * LINE_INK,
            ],
            1.0,
        ));
        let ink = crate::kit::linear(crate::kit::accent_on(dark));
        g.labels.push(Label {
            line_px: LINE_PX,
            ..centred(
                &words(Ctl::Main, prefs),
                b.x + b.w / 2.0,
                b.y + (b.h - LINE_PX) / 2.0,
                FONT_PX,
                [ink[0], ink[1], ink[2], 1.0],
                Some(FONT_MEDIUM),
                b,
            )
        });
        scene.grids.push(g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: (f32, f32) = (1440.0, 900.0);

    #[test]
    fn the_button_stands_under_the_selections_bottom_right_corner() {
        let sel = Rect::new(300.0, 100.0, 640.0, 400.0);
        let b = button_place(sel, 80.0, ROOM, 380.0);
        assert_eq!(
            (b.x + b.w, b.y),
            (940.0, 508.0),
            "its right edge in line, just under"
        );
        assert_eq!((b.w, b.h), (80.0, BUTTON_H));
    }

    #[test]
    fn with_no_room_below_it_stands_inside_over_that_corner() {
        // A selection that reaches the screen's bottom (a window, the screen).
        let sel = Rect::new(0.0, 28.0, 1440.0, 872.0);
        let b = button_place(sel, 80.0, ROOM, -BY);
        assert!(b.y + b.h <= 900.0 - BY + 0.5 && b.y >= sel.y, "{b:?}");
        assert!(b.x + b.w <= 1440.0 - BY + 0.5, "clear of the edge: {b:?}");
    }

    #[test]
    fn on_a_narrow_selection_it_begins_after_the_size_and_never_leaves_the_screen() {
        // 40 wide: the size's plate (to x 190) is wider than the selection.
        let sel = Rect::new(100.0, 100.0, 40.0, 40.0);
        let b = button_place(sel, 80.0, ROOM, 190.0);
        assert_eq!(b.x, 198.0);
        // At the screen's right edge it is pulled back onto it.
        let sel = Rect::new(1400.0, 100.0, 40.0, 40.0);
        let b = button_place(sel, 80.0, ROOM, 1490.0);
        assert_eq!(b.x + b.w, 1440.0);
        // At the left edge too.
        let b = button_place(Rect::new(0.0, 100.0, 30.0, 30.0), 80.0, ROOM, -BY);
        assert_eq!(b.x, 0.0);
    }
}

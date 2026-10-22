//! THE SHORTCUTS TABLE (Max, 2026-10-22): `?` in keyboard mode — or a right
//! click on its button — shows every key a file window answers, in the
//! confirmation's material (the menus' paint), centred over the window:
//! two columns of rows, each a small key cap and what it does, under the
//! headings "Keyboard mode" and "Always". `?`, Escape or a press anywhere
//! puts it away.

use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_MEDIUM};
use crate::desktop_menu::MenuPaint;
use crate::i18n::tr;

/// A line of the table: a heading, or keys and what they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Heading(&'static str),
    Keys(&'static str, &'static str),
}

/// Every line, in reading order (the columns are cut where they balance).
const LINES: &[Line] = &[
    Line::Heading("Keyboard mode"),
    Line::Keys("W A S D", "Move around"),
    Line::Keys("Shift+W A S D", "Select a run"),
    Line::Keys("E", "Open"),
    Line::Keys("Q", "Back"),
    Line::Keys("U", "Up"),
    Line::Keys("/", "Golem Disk"),
    Line::Keys(".", "Home"),
    Line::Keys("1–9", "A step of the path"),
    Line::Keys("← →", "Previous or next tab"),
    Line::Keys("Z", "Undo"),
    Line::Keys("X", "Cut"),
    Line::Keys("C", "Copy"),
    Line::Keys("V", "Paste"),
    Line::Keys("R", "Rename"),
    Line::Keys("N", "New folder"),
    Line::Keys("F", "Search"),
    Line::Keys("G", "First"),
    Line::Keys("Shift+G", "Last"),
    Line::Keys("I", "Get info"),
    Line::Keys("H", "Hidden files"),
    Line::Keys("L", "Grid or list"),
    Line::Keys("T", "New tab"),
    Line::Keys("Shift+T", "Close tab"),
    Line::Keys("Delete", "Move to bin"),
    Line::Keys("?", "Keyboard shortcuts"),
    Line::Heading("Always"),
    Line::Keys("Ctrl+C X V", "Copy, cut, paste"),
    Line::Keys("Ctrl+Z", "Undo"),
    Line::Keys("Ctrl+F", "Search"),
    Line::Keys("Ctrl+L", "Type a path"),
    Line::Keys("Ctrl+H", "Hidden files"),
    Line::Keys("Ctrl+T", "New tab"),
    Line::Keys("Ctrl+W", "Close tab"),
    Line::Keys("Ctrl+Tab", "Next tab"),
    Line::Keys("Space", "Quick look"),
    Line::Keys("Enter", "Open"),
    Line::Keys("Esc", "Close or clear"),
    Line::Keys("F2", "Rename"),
    Line::Keys("Shift+F10", "Menu"),
    Line::Keys("Ctrl + − 0", "Menu size"),
];

/// The panel's width, air, rows and words (the menus' sizes).
const W: f32 = 520.0;
const PAD: f32 = 18.0;
const COL_GAP: f32 = 24.0;
const ROW_H: f32 = 22.0;
const HEAD_H: f32 = 26.0;
const HEAD_PX: f32 = 11.5;
const TEXT_PX: f32 = 12.5;
const TEXT_LINE: f32 = 16.0;
/// A key cap: its height, air, corner and words; the gap to its words.
const CAP_H: f32 = 17.0;
const CAP_PAD: f32 = 5.0;
const CAP_RADIUS: f32 = 4.0;
const CAP_PX: f32 = 11.0;
const CAP_GAP: f32 = 8.0;
/// The widest key cap a column keeps room for.
const CAP_COL: f32 = 104.0;
/// A cap's fill: the panel a step toward the ink; a heading's ink.
const CAP_FILL: f32 = 0.09;
const HEAD_INK: f32 = 0.55;
/// How fast it comes up.
const RATE: f32 = 14.0;

/// The table that is up.
#[derive(Debug, Clone)]
pub(crate) struct Shortcuts {
    /// Where the second column begins (an index of `LINES`).
    cut: usize,
    /// The menus' size when it opened.
    scale: f32,
    /// The panel, in the window's space.
    pub rect: Rect,
    /// How far it has come up (0..1).
    pub t: f32,
}

/// A line's height.
fn line_h(l: &Line) -> f32 {
    match l {
        Line::Heading(_) => HEAD_H,
        Line::Keys(..) => ROW_H,
    }
}

impl Shortcuts {
    /// The table, centred in a window `size` big.
    pub fn new(size: (f32, f32)) -> Self {
        // (Cut where the two columns are nearest each other's height; never
        // a heading left alone at a column's foot.)
        let total: f32 = LINES.iter().map(line_h).sum();
        let mut acc = 0.0;
        let mut cut = LINES.len();
        for (i, l) in LINES.iter().enumerate() {
            if acc + line_h(l) / 2.0 >= total / 2.0 {
                cut = if matches!(LINES.get(i.saturating_sub(1)), Some(Line::Heading(_))) {
                    i - 1
                } else {
                    i
                };
                break;
            }
            acc += line_h(l);
        }
        let mut s = Self {
            cut,
            scale: crate::desktop_menu::scale(),
            rect: Rect::default(),
            t: 0.0,
        };
        s.place(size);
        s
    }

    /// Centred again in a window `size` big.
    pub fn place(&mut self, (w, h): (f32, f32)) {
        let s = self.scale;
        let col = |lines: &[Line]| lines.iter().map(line_h).sum::<f32>();
        let tall = col(&LINES[..self.cut]).max(col(&LINES[self.cut..]));
        let (pw, ph) = (W * s, (2.0 * PAD + tall) * s);
        self.rect = Rect::new(((w - pw) / 2.0).max(0.0), ((h - ph) / 2.0).max(0.0), pw, ph);
    }

    /// One step of its coming up; whether it is still moving.
    pub fn ease(&mut self, dt: f32) -> bool {
        let (t, moving) = crate::animation::ease_toward(self.t, 1.0, dt, RATE, 0.01);
        self.t = t;
        moving
    }

    /// Drawn over everything (`measure`: a text's width at a size).
    pub fn push(
        &self,
        scene: &mut Scene,
        paint: &MenuPaint,
        measure: &mut dyn FnMut(&str, f32) -> f32,
    ) {
        let s = self.scale;
        let t = self.t.clamp(0.0, 1.0);
        let fade = |c: [f32; 4], a: f32| [c[0], c[1], c[2], c[3] * a * t];
        let ink = paint.ink;
        let r = Rect::new(
            self.rect.x,
            self.rect.y + (1.0 - t) * 6.0 * s,
            self.rect.w,
            self.rect.h,
        );
        let mut g = GridContent {
            clip: r,
            ..Default::default()
        };
        g.rects.push(RectInst {
            rect: r,
            radius: paint.radius * s,
            color: fade(paint.fill, 1.0),
            glass: paint.glass,
            border: 0.0,
        });
        let cap_fill = mix(paint.fill, ink, CAP_FILL);
        let col_w = (r.w - 2.0 * PAD * s - COL_GAP * s) / 2.0;
        let label =
            |text: &str, (x, y): (f32, f32), max_w: f32, px: f32, medium: bool, color: [f32; 4]| {
                Label {
                    text: text.to_owned(),
                    pos: (x, y),
                    max_w,
                    font_px: px,
                    line_px: px + 4.0,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: Some(r),
                    family: medium.then_some(FONT_MEDIUM),
                    color: Some(color),
                }
            };
        for (c, lines) in [&LINES[..self.cut], &LINES[self.cut..]]
            .into_iter()
            .enumerate()
        {
            let x = r.x + PAD * s + c as f32 * (col_w + COL_GAP * s);
            let mut y = r.y + PAD * s;
            for l in lines {
                match *l {
                    Line::Heading(words) => {
                        let top = y + (HEAD_H * s - (HEAD_PX + 4.0) * s) / 2.0 + 2.0 * s;
                        g.labels.push(label(
                            tr(words),
                            (x, top),
                            col_w,
                            HEAD_PX * s,
                            true,
                            fade(ink, HEAD_INK),
                        ));
                    }
                    Line::Keys(keys, words) => {
                        let cap_w =
                            (measure(keys, CAP_PX * s) + 2.0 * CAP_PAD * s).min(CAP_COL * s);
                        let cap = Rect::new(x, y + (ROW_H - CAP_H) / 2.0 * s, cap_w, CAP_H * s);
                        g.rects.push(RectInst {
                            rect: cap,
                            radius: CAP_RADIUS * s,
                            color: fade(cap_fill, 1.0),
                            glass: 0.0,
                            border: 0.0,
                        });
                        g.labels.push(Label {
                            clip: Some(cap),
                            ..label(
                                keys,
                                (
                                    cap.x + CAP_PAD * s,
                                    cap.y + (cap.h - (CAP_PX + 4.0) * s) / 2.0,
                                ),
                                cap.w,
                                CAP_PX * s,
                                false,
                                fade(ink, 0.9),
                            )
                        });
                        let wx = x + (CAP_COL + CAP_GAP) * s;
                        g.labels.push(label(
                            tr(words),
                            (wx, y + (ROW_H - TEXT_LINE) / 2.0 * s),
                            x + col_w - wx,
                            TEXT_PX * s,
                            false,
                            fade(ink, 0.86),
                        ));
                    }
                }
                y += line_h(l) * s;
            }
        }
        scene.grids.push(g);
    }
}

/// `a` mixed `k` of the way toward `b`.
fn mix(a: [f32; 4], b: [f32; 4], k: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * k,
        a[1] + (b[1] - a[1]) * k,
        a[2] + (b[2] - a[2]) * k,
        a[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_columns_balance_and_a_heading_never_ends_one() {
        let t = Shortcuts::new((1200.0, 900.0));
        let col = |lines: &[Line]| lines.iter().map(line_h).sum::<f32>();
        let (a, b) = (col(&LINES[..t.cut]), col(&LINES[t.cut..]));
        assert!((a - b).abs() <= HEAD_H + ROW_H, "{a} vs {b}");
        assert!(!matches!(LINES[t.cut - 1], Line::Heading(_)));
        // Centred, about 520 wide.
        assert!((t.rect.x + t.rect.w / 2.0 - 600.0).abs() < 1.0);
        assert!((t.rect.w - 520.0).abs() < 1.0);
    }

    #[test]
    fn every_word_has_its_spanish() {
        let es: std::collections::HashMap<String, String> =
            serde_json::from_str(include_str!("../../assets/i18n/es.json")).unwrap();
        for l in LINES {
            let words = match l {
                Line::Heading(w) | Line::Keys(_, w) => w,
            };
            assert!(es.contains_key(*words), "no Spanish for {words:?}");
        }
    }
}

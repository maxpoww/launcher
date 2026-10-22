//! THE CONFIRMATION (Max, 2026-10-22: "add a confirmation for changes made
//! on specific directories"): a change that touches a DELICATE place
//! (`zones`) asks first — a small panel in the menus' material, centred
//! over the file window: a shield, a title naming the act ("Move “.config”
//! to the bin?"), ONE plain sentence for that place, and [Cancel] [<the act>
//! anyway]. Cancel has the focus; Enter or Space press the focused button,
//! Tab and the arrows move it, Escape cancels. One panel an action, however
//! many things it covers.

use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene, FONT_MEDIUM};
use crate::desktop_menu::MenuPaint;
use smithay_client_toolkit::seat::keyboard::Keysym;

/// The panel's width, its air, the shield, the words' sizes, the buttons'.
const W: f32 = 360.0;
const PAD: f32 = 18.0;
const ICON: f32 = 18.0;
const ICON_GAP: f32 = 12.0;
const TITLE_PX: f32 = 14.0;
const TITLE_LINE: f32 = 19.0;
const TEXT_PX: f32 = 12.5;
const TEXT_LINE: f32 = 17.0;
const SAY_GAP: f32 = 6.0;
const BTN_H: f32 = 28.0;
const BTN_PAD: f32 = 14.0;
const BTN_GAP: f32 = 8.0;
const BTN_TOP: f32 = 18.0;
const BTN_RADIUS: f32 = 6.0;
/// The buttons at rest: the panel a step toward the ink.
const BTN_REST: f32 = 0.07;
/// The panel's hairline (the menus' rules).
const LINE_INK: f32 = 0.14;
/// How fast it comes up.
const RATE: f32 = 14.0;

/// What a key or a click did to the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Nothing,
    /// Its focus or hover moved: drawn again.
    Moved,
    Cancel,
    Go,
}

/// A confirmation that is up.
#[derive(Debug, Clone)]
pub(crate) struct Confirm {
    /// Its title and its sentence, in lines as they fit.
    title: Vec<String>,
    say: Vec<String>,
    /// The second button's words ("Move to bin anyway").
    go: String,
    /// The focused button (0 Cancel, 1 the act) and the one under the pointer.
    pub focus: usize,
    pub hover: Option<usize>,
    /// Its buttons' widths, and its size (the menus' size at the time).
    btn_w: [f32; 2],
    scale: f32,
    /// The panel, in the window's space.
    pub rect: Rect,
    /// How far it has come up (0..1).
    pub t: f32,
}

impl Confirm {
    /// The panel for `title`, `say` and the act's words `go`, centred in a
    /// window `size` big (`measure`: a text's width at a size, in the
    /// title's weight or not).
    pub fn new(
        title: &str,
        say: &str,
        go: &str,
        size: (f32, f32),
        measure: &mut dyn FnMut(&str, f32, bool) -> f32,
    ) -> Self {
        let s = crate::desktop_menu::scale();
        let words_w = (W - 2.0 * PAD - ICON - ICON_GAP) * s;
        let title = wrap(title, words_w, &mut |t| measure(t, TITLE_PX * s, true));
        let say = wrap(say, words_w, &mut |t| measure(t, TEXT_PX * s, false));
        let btn = |t: &str, m: &mut dyn FnMut(&str, f32, bool) -> f32| {
            m(t, TEXT_PX * s, false) + 2.0 * BTN_PAD * s
        };
        let btn_w = [btn(crate::i18n::tr("Cancel"), measure), btn(go, measure)];
        let mut c = Self {
            title,
            say,
            go: go.to_owned(),
            focus: 0,
            hover: None,
            btn_w,
            scale: s,
            rect: Rect::default(),
            t: 0.0,
        };
        c.place(size);
        c
    }

    /// Centred again in a window `size` big.
    pub fn place(&mut self, (w, h): (f32, f32)) {
        let s = self.scale;
        let words =
            self.title.len() as f32 * TITLE_LINE + SAY_GAP + self.say.len() as f32 * TEXT_LINE;
        let ph = (2.0 * PAD + words.max(ICON) + BTN_TOP + BTN_H) * s;
        let pw = (W * s).max(self.btn_w[0] + self.btn_w[1] + BTN_GAP * s + 2.0 * PAD * s);
        self.rect = Rect::new(((w - pw) / 2.0).max(0.0), ((h - ph) / 2.0).max(0.0), pw, ph);
    }

    /// The two buttons, Cancel then the act, at the panel's foot on the right.
    pub fn buttons(&self) -> [Rect; 2] {
        let s = self.scale;
        let r = self.rect;
        let y = r.y + r.h - (PAD + BTN_H) * s;
        let go = Rect::new(
            r.x + r.w - PAD * s - self.btn_w[1],
            y,
            self.btn_w[1],
            BTN_H * s,
        );
        let cancel = Rect::new(
            go.x - BTN_GAP * s - self.btn_w[0],
            y,
            self.btn_w[0],
            BTN_H * s,
        );
        [cancel, go]
    }

    /// The button at `at`, if any.
    pub fn hit(&self, at: (f32, f32)) -> Option<usize> {
        self.buttons().iter().position(|b| b.contains(at))
    }

    /// The pointer at `at` (or gone): which button it is on.
    pub fn pointer(&mut self, at: Option<(f32, f32)>) -> bool {
        let now = at.and_then(|p| self.hit(p));
        let moved = now != self.hover;
        self.hover = now;
        moved
    }

    /// A key while it is up.
    pub fn key(&mut self, keysym: Keysym) -> Answer {
        match keysym {
            Keysym::Escape => Answer::Cancel,
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if self.focus == 1 {
                    Answer::Go
                } else {
                    Answer::Cancel
                }
            }
            Keysym::Tab | Keysym::ISO_Left_Tab | Keysym::Left | Keysym::Right => {
                self.focus = 1 - self.focus;
                Answer::Moved
            }
            _ => Answer::Nothing,
        }
    }

    /// One step of its coming up; whether it is still moving.
    pub fn ease(&mut self, dt: f32) -> bool {
        let (t, moving) = crate::animation::ease_toward(self.t, 1.0, dt, RATE, 0.01);
        self.t = t;
        moving
    }

    /// Drawn over everything (`shield`: the shield's layer, if it is in).
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint, shield: Option<u32>) {
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
        let solid = |rect: Rect, radius: f32, color: [f32; 4], border: f32| RectInst {
            rect,
            radius,
            color,
            glass: paint.glass,
            border,
        };
        g.rects
            .push(solid(r, paint.radius * s, fade(paint.fill, 1.0), 0.0));
        g.rects
            .push(solid(r, paint.radius * s, fade(ink, LINE_INK), 1.0));
        let x = r.x + PAD * s;
        let top = r.y + PAD * s;
        if let Some(layer) = shield {
            g.icons.push(IconInst {
                rect: Rect::new(x, top, ICON * s, ICON * s),
                layer,
                tint: [ink[0], ink[1], ink[2], 1.0],
                ring: -1.17,
                plate: [0.0, 0.0, 0.0, 0.8 * t],
            });
        }
        let words_x = x + (ICON + ICON_GAP) * s;
        let label = |text: &str, y: f32, px: f32, line: f32, bold: bool, color: [f32; 4]| Label {
            text: text.to_owned(),
            pos: (words_x, y),
            max_w: r.x + r.w - PAD * s - words_x + 2.0,
            font_px: px,
            line_px: line,
            centered: false,
            dim: false,
            cache: true,
            clip: Some(r),
            family: bold.then_some(FONT_MEDIUM),
            color: Some(color),
        };
        let mut y = top;
        for line in &self.title {
            g.labels.push(label(
                line,
                y,
                TITLE_PX * s,
                TITLE_LINE * s,
                true,
                fade(ink, 1.0),
            ));
            y += TITLE_LINE * s;
        }
        y += SAY_GAP * s;
        for line in &self.say {
            g.labels.push(label(
                line,
                y,
                TEXT_PX * s,
                TEXT_LINE * s,
                false,
                fade(ink, 0.78),
            ));
            y += TEXT_LINE * s;
        }
        // The buttons: the panel a step toward the ink; the focused one, or
        // the one under the pointer, tinted toward the accent as a menu's row.
        let rest = mix(paint.fill, ink, BTN_REST);
        let lit = paint.hover(paint.accent, 0.20, 0.28, ink);
        let lift = r.y - self.rect.y;
        for (n, (b, words)) in self
            .buttons()
            .into_iter()
            .zip([crate::i18n::tr("Cancel"), self.go.as_str()])
            .enumerate()
        {
            let b = Rect::new(b.x, b.y + lift, b.w, b.h);
            let on = self.focus == n || self.hover == Some(n);
            g.rects.push(solid(
                b,
                BTN_RADIUS * s,
                fade(if on { lit } else { rest }, 1.0),
                0.0,
            ));
            // (The focus is also a ring: the keys' place is never a guess.)
            if self.focus == n {
                g.rects
                    .push(solid(b, BTN_RADIUS * s, fade(paint.accent, 0.9), 1.0));
            }
            g.labels.push(Label {
                pos: (b.x + BTN_PAD * s, b.y + (b.h - TEXT_LINE * s) / 2.0),
                max_w: b.w,
                clip: Some(b),
                ..label(
                    words,
                    0.0,
                    TEXT_PX * s,
                    TEXT_LINE * s,
                    false,
                    fade(ink, 1.0),
                )
            });
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

/// `text` in lines no wider than `max_w`, broken between words.
pub(crate) fn wrap(text: &str, max_w: f32, measure: &mut dyn FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let tried = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && measure(&tried) > max_w {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = tried;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_wrapped_between_words() {
        // (Each letter 10 wide.)
        let mut m = |t: &str| t.chars().count() as f32 * 10.0;
        assert_eq!(
            wrap("It holds your apps' settings.", 120.0, &mut m),
            vec!["It holds", "your apps'", "settings."]
        );
        assert_eq!(wrap("", 100.0, &mut m), Vec::<String>::new());
        // A word longer than the line stands alone.
        assert_eq!(
            wrap("a verylongwordindeed b", 60.0, &mut m),
            vec!["a", "verylongwordindeed", "b"]
        );
    }

    #[test]
    fn cancel_has_the_focus_and_the_keys_move_it() {
        let mut m = |t: &str, px: f32, _: bool| t.chars().count() as f32 * px * 0.5;
        let mut c = Confirm::new(
            "Move “.config” to the bin?",
            "It holds your apps' settings. Apps can lose them.",
            "Move to bin anyway",
            (1000.0, 700.0),
            &mut m,
        );
        assert_eq!(c.focus, 0);
        assert_eq!(c.key(Keysym::Return), Answer::Cancel);
        assert_eq!(c.key(Keysym::Tab), Answer::Moved);
        assert_eq!(c.key(Keysym::space), Answer::Go);
        assert_eq!(c.key(Keysym::Escape), Answer::Cancel);
        // Centred, its buttons inside it, the act's on the right.
        let [cancel, go] = c.buttons();
        assert!((c.rect.x + c.rect.w / 2.0 - 500.0).abs() < 1.0);
        assert!(cancel.x + cancel.w < go.x && go.x + go.w <= c.rect.x + c.rect.w);
        assert_eq!(c.hit((go.x + 2.0, go.y + 2.0)), Some(1));
        assert_eq!(c.hit((c.rect.x + 2.0, c.rect.y + 2.0)), None);
    }
}

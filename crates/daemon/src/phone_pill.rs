//! The PHONE pill: on the OPTIONS bar for as long as a phone is plugged in —
//! Max, 2026-10-11: *"a proper phones pill, not only for the cam but the
//! whole menu. start by making a round pill, that opens a long pill on hover
//! (as notification), place it to the left of notif, the pill shows up when
//! the phone is connected, and the long one shows the phone status (name:
//! sync and configured)… and on click on the round pill, eject the phone."*
//!
//! AT REST: a round pill with a phone on it, left of the bell.
//!
//! THE POINTER ON IT: a long pill comes out from behind it, to the left (the
//! way the bell's preview comes out from behind the bell), saying whose
//! phone it is and how it stands with this computer: `Max's Pixel · Synced
//! 2 h ago`, `· Syncing…`, `· Sync off`, `· Not set up` (never configured
//! here), `· Locked` (plugged in but not open).
//!
//! A CLICK on the round pill ejects the phone.
//!
//! It grows into the ground left of itself and pushes nothing: the task
//! pill's place by the bell and the camera pill's right end both stop short
//! of it (`phone_pill_reserve`).

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp, lerp4};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-mobile.
const GLYPH_PHONE: &str = "\u{f10b}";

/// The button in the long pill's air: what a click on it does.
const SYNC: &str = "Sync";
const SET_UP: &str = "Set up";
/// The least air either side of that button.
const CHIP_AIR: f32 = 10.0;

/// Between the battery and its number.
const BATTERY_GAP: f32 = 5.0;
/// The battery's glyph beside the words' size: the icon font's glyphs are
/// held to one narrow cell, and at the words' size it was a speck.
const BATTERY_SIZE: f32 = 1.7;

/// fa-battery-full … fa-battery-empty: the one that is as full as `level`.
fn battery_glyph(level: u8) -> &'static str {
    match level {
        88..=u8::MAX => "\u{f240}",
        63..=87 => "\u{f241}",
        38..=62 => "\u{f242}",
        13..=37 => "\u{f243}",
        _ => "\u{f244}",
    }
}

/// How much longer than its words need the long pill is.
const LONGER: f32 = 4.0 / 3.0;

/// What the pill shows.
#[derive(Debug, Clone, PartialEq, Default)]
struct Shown {
    /// The volume's path (what is ejected).
    path: String,
    /// "Max's Pixel · Synced 2 h ago".
    says: String,
    /// Its battery in hundredths, once it has said: a battery and the
    /// number, after the words.
    battery: Option<u8>,
    /// What `adb -s` is told for it (`None`: it has no adb to ask).
    serial: Option<String>,
    /// It is known here (in `phones.json`).
    known: bool,
    /// The word on its button (`None`: none now — it is being synced, or
    /// it is locked).
    button: Option<&'static str>,
}

/// The pill's state.
#[derive(Debug, Default)]
pub(crate) struct PhonePill {
    /// 0 (no phone) → 1 (there).
    t: f32,
    /// 0 (the round pill) → 1 (the long one out).
    open: f32,
    want_open: bool,
    last: Option<Instant>,
    frame_pending: bool,
    /// Kept after the phone is gone, for the pill's way out.
    shown: Option<Shown>,
    /// The words' measured width, and what was measured.
    says_w: f32,
    measured: String,
    /// The battery's glyph and its number, measured.
    glyph_w: f32,
    level_w: f32,
    /// The button's word, measured (0: no button now).
    chip_w: f32,
    /// A phone that is not there (`debug-desktop "phonepill fake"`), and the
    /// pill stood open or shut without a pointer.
    fake: Option<Shown>,
    forced: Option<bool>,
    /// Each phone's battery as it last said (by serial), and the one being
    /// asked right now.
    battery: std::collections::HashMap<String, u8>,
    asking: Option<String>,
}

/// "5 min ago", "2 h ago", "3 days ago".
fn ago(secs: u64) -> String {
    match secs {
        0..=89 => "just now".to_owned(),
        90..=5399 => format!("{} min ago", (secs + 30) / 60),
        5400..=129_599 => format!("{} h ago", (secs + 1800) / 3600),
        _ => format!("{} days ago", (secs + 43_200) / 86_400),
    }
}

impl App {
    /// The phone that is plugged in, and what there is to say of it.
    fn phone_pill_now(&self) -> Option<Shown> {
        if let Some(fake) = &self.phone_pill.fake {
            return Some(fake.clone());
        }
        let v = self.desktop.volumes.iter().find(|v| v.phone)?;
        let serial = crate::desktop::phone_serial_of(v);
        let known = serial.as_ref().and_then(|s| crate::phones::Phones::load().by_serial.get(s).cloned());
        let name = known.as_ref().map_or_else(|| v.name.clone(), |p| p.name.clone());
        let syncing = serial.as_ref().is_some_and(|s| self.desktop.syncing.contains(s));
        let state = match &known {
            _ if v.closed => "Locked".to_owned(),
            _ if syncing => "Syncing…".to_owned(),
            None => "Not set up".to_owned(),
            Some(p) if !p.sync => "Sync off".to_owned(),
            Some(p) => match p.last_sync {
                Some(at) => format!("Synced {}", ago(crate::desktop_config::now_secs().saturating_sub(at))),
                None => "Not synced yet".to_owned(),
            },
        };
        // Its battery, once it has said (Max, 2026-10-11: "the battery on
        // the big pill").
        let battery = serial.as_ref().and_then(|s| self.phone_pill.battery.get(s)).copied();
        Some(Shown {
            path: v.path.to_string_lossy().into_owned(),
            says: format!("{name} · {state}"),
            battery,
            button: match &known {
                _ if v.closed || syncing => None,
                None => Some(SET_UP),
                Some(_) => Some(SYNC),
            },
            known: known.is_some(),
            serial,
        })
    }

    /// Ask the plugged-in phone how full its battery is (off the loop); the
    /// pill is written again when it answers.
    fn phone_pill_ask_battery(&mut self) {
        let Some(serial) = self.phone_pill_now().and_then(|s| s.serial) else {
            return;
        };
        if self.phone_pill.fake.is_some() || self.phone_pill.asking.as_ref() == Some(&serial) {
            return;
        }
        self.phone_pill.asking = Some(serial.clone());
        self.desktop_off_loop(
            move || {
                let level = crate::desktop_phone::battery(&serial);
                (serial, level)
            },
            |app, (serial, level)| {
                app.phone_pill.asking = None;
                if let Some(level) = level {
                    if app.phone_pill.battery.insert(serial, level) != Some(level) {
                        app.phone_pill_sync();
                    }
                }
            },
        );
    }

    /// A phone came or went, or what there is to say of it may have changed:
    /// the pill follows.
    pub(crate) fn phone_pill_sync(&mut self) {
        if let Some(now) = self.phone_pill_now() {
            let key = format!("{}{:?}{:?}", now.says, now.battery, now.button);
            if key != self.phone_pill.measured {
                let font_px = FONT_PX * self.options_scale();
                if let Some(r) = self.options_renderer.as_mut() {
                    self.phone_pill.says_w = r.measure_text(&now.says, font_px, TEXT_FONT);
                    (self.phone_pill.glyph_w, self.phone_pill.level_w) = match now.battery {
                        Some(b) => (
                            r.measure_text(battery_glyph(b), font_px * BATTERY_SIZE, Some(crate::options::NERD)),
                            r.measure_text(&format!("{b}%"), font_px, TEXT_FONT),
                        ),
                        None => (0.0, 0.0),
                    };
                    self.phone_pill.chip_w = now.button.map_or(0.0, |word| r.measure_text(word, font_px, TEXT_FONT) + 12.0);
                    self.phone_pill.measured = key;
                }
            }
            // A phone not asked yet (it has just been plugged in).
            if now.serial.as_ref().is_some_and(|s| !self.phone_pill.battery.contains_key(s)) {
                self.phone_pill_ask_battery();
            }
            self.phone_pill.shown = Some(now);
        }
        self.schedule_phone_pill_frame();
        self.draw_options();
    }

    fn schedule_phone_pill_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.phone_pill.frame_pending, &mut a.phone_pill.last), App::tick_phone_pill);
    }

    /// One frame of its coming, opening and going.
    fn tick_phone_pill(&mut self) {
        let now = Instant::now();
        let dt = self.phone_pill.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.phone_pill.last = Some(now);
        // (Asked every frame of its motion: only whether one is plugged in,
        // not all there is to say of it.)
        let plugged = self.phone_pill.fake.is_some() || self.desktop.volumes.iter().any(|v| v.phone);
        let there = f32::from(u8::from(plugged));
        let settle = animation::settle_t(self.phone_pill_long_w().max(1.0));
        let p = &self.phone_pill;
        let (t, a) = ease_toward(p.t, there, dt, animation::MORPH_RATE, settle);
        let want = f32::from(u8::from(p.want_open && there > 0.0));
        let (open, b) = ease_toward(p.open, want, dt, animation::MORPH_RATE, settle);
        self.phone_pill.t = t;
        self.phone_pill.open = open;
        self.sync_options_input();
        self.draw_options();
        if a || b {
            self.schedule_phone_pill_frame();
        } else {
            self.phone_pill.last = None;
            if there == 0.0 {
                self.phone_pill.shown = None;
                self.phone_pill.measured.clear();
            }
        }
    }

    /// The long pill's width: its words and the pills' padding, and a
    /// third as much again (Max, 2026-10-11: "make the long pill 1/3
    /// longer") — the words stand in its middle.
    fn phone_pill_long_w(&self) -> f32 {
        let base = self.phone_pill_words_w() + 2.0 * PILL_PAD_X;
        // (The air it leaves in its middle holds the button.)
        let chip = self.phone_pill.chip_w;
        (base * LONGER).max(base + if chip > 0.0 { chip + 2.0 * CHIP_AIR } else { 0.0 })
    }

    /// The button in the long pill's air (Max, 2026-10-11: "a sync button
    /// on the air between the status and the battery"): the bell's amber
    /// chip, in the middle of what the words and the battery leave.
    fn phone_pill_chip(&self, long: Rect) -> Option<Rect> {
        let p = &self.phone_pill;
        if p.chip_w <= 0.0 {
            return None;
        }
        let battery_w = if p.level_w > 0.0 { p.glyph_w + BATTERY_GAP + p.level_w } else { 0.0 };
        let (left, right) = (long.x + PILL_PAD_X + p.says_w, long.x + long.w - PILL_PAD_X - battery_w);
        let h = LINE_PX * self.options_scale();
        (right - left >= p.chip_w).then(|| Rect::new((left + right - p.chip_w) / 2.0, long.y + (long.h - h) / 2.0, p.chip_w, h))
    }

    /// The long pill within the whole, fully out.
    fn phone_pill_long(&self, rect: Rect) -> Rect {
        let round = Self::phone_pill_round(rect);
        Rect::new(rect.x, rect.y, (rect.w - round.w - OPTION_GAP * self.phone_pill.open).max(0.0), rect.h)
    }

    /// All the long pill writes, end to end: the words, and after them the
    /// battery and its number.
    fn phone_pill_words_w(&self) -> f32 {
        let p = &self.phone_pill;
        let battery = if p.level_w > 0.0 { p.glyph_w + BATTERY_GAP + p.level_w } else { 0.0 };
        p.says_w + battery
    }

    /// How much of the bar left of the bell the round pill takes (what the
    /// pills that stand by the bell stop short of).
    pub(crate) fn phone_pill_reserve(&self) -> f32 {
        if self.phone_pill.shown.is_none() {
            return 0.0;
        }
        (self.options_pill_h() + OPTION_GAP) * self.phone_pill.t.clamp(0.0, 1.0)
    }

    /// Where the pill is on the bar now (`None`: no phone): the round pill
    /// left of the bell (`bell_left`), and left of it as much of the long
    /// one as is out.
    pub(crate) fn phone_pill_rect(&self, bell_left: f32, y: f32, ph: f32) -> Option<Rect> {
        let p = &self.phone_pill;
        if p.t <= 0.01 || p.shown.is_none() {
            return None;
        }
        let right = bell_left - OPTION_GAP;
        let long = (OPTION_GAP + self.phone_pill_long_w()) * p.open;
        Some(Rect::new(right - ph - long, y, ph + long, ph))
    }

    /// The round pill within the whole.
    fn phone_pill_round(rect: Rect) -> Rect {
        Rect::new(rect.x + rect.w - rect.h, rect.y, rect.h, rect.h)
    }

    /// The pointer came onto the pill or went off it (called on every hover
    /// move over the bar). Whether the pointer is on it.
    pub(crate) fn update_phone_hover(&mut self) -> bool {
        let on = self.options_hover == Some(PillId::Phone);
        let want = self.phone_pill.forced.unwrap_or(on);
        if want != self.phone_pill.want_open {
            self.phone_pill.want_open = want;
            // (What it says is read as it opens: "Synced 2 h ago" ages, and
            // so does its battery.)
            if want {
                self.phone_pill_sync();
                self.phone_pill_ask_battery();
            }
            self.schedule_phone_pill_frame();
        }
        on
    }

    /// Its long pill is out (it stands over what was there).
    pub(crate) fn phone_pill_out(&self) -> bool {
        self.phone_pill.open > 0.05
    }

    /// The pointer left the bar: the long pill goes back in.
    pub(crate) fn phone_pill_close(&mut self) {
        if self.phone_pill.forced.is_none() && self.phone_pill.want_open {
            self.phone_pill.want_open = false;
            self.schedule_phone_pill_frame();
        }
    }

    /// A click on the pill: on the round one, the phone is ejected; on the
    /// long one, it is synced now (Max, 2026-10-11: "click on the big pill
    /// triggers a sync") — a phone never set up here gets its Configure box
    /// instead, there being nothing yet to sync it by.
    pub(crate) fn phone_pill_click(&mut self) {
        let (Some(ptr), Some(rect)) = (self.options_ptr, self.phone_pill_on_bar()) else {
            return;
        };
        if self.phone_pill.fake.is_some() {
            return;
        }
        let round = Self::phone_pill_round(rect);
        if ptr.0 < round.x {
            self.phone_pill_sync_now((rect.x, rect.y + rect.h + 8.0));
            return;
        }
        if let Some(path) = self.phone_pill.shown.as_ref().map(|s| s.path.clone()) {
            tracing::info!("desktop: eject from the bar's phone pill");
            self.desktop_eject(&path);
        }
    }

    /// Sync the plugged-in phone now, by its cable, whatever its settings
    /// say of when. `at`: where a Configure box would open.
    fn phone_pill_sync_now(&mut self, at: (f32, f32)) {
        let Some(shown) = self.phone_pill_now() else {
            return;
        };
        if !shown.known {
            if let Some(i) = self.desktop.items.iter().position(|it| it.path == shown.path) {
                self.desktop_open_config(i, at);
            }
            return;
        }
        let Some(serial) = shown.serial else {
            return;
        };
        let Some(phone) = crate::phones::Phones::load().by_serial.get(&serial).cloned() else {
            return;
        };
        tracing::info!("desktop: {} synced now, from the bar's phone pill", phone.name);
        self.phone_sync(serial.clone(), phone, serial);
        self.phone_pill_sync();
    }

    /// The pill driven without a pointer (`debug-desktop "phonepill …"`):
    /// `fake [words]` (a phone that is not there) / `gone`, `open` / `close`
    /// (stood so) / `free` (the pointer's again). Never a click: that ejects.
    pub(crate) fn phone_pill_debug(&mut self, what: &str) -> String {
        let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
        match verb {
            "fake" => {
                // (`fake 35 Some words`: a battery at 35.)
                let (level, words) = match rest.trim().split_once(' ') {
                    Some((n, words)) if n.parse::<u8>().is_ok() => (n.parse::<u8>().ok(), words.trim()),
                    _ => (Some(66), rest.trim()),
                };
                let words = if words.is_empty() { "Max's Pixel · Synced 2 h ago" } else { words };
                self.phone_pill.fake = Some(Shown {
                    path: "/nowhere/a-phone".to_owned(),
                    says: words.to_owned(),
                    battery: level,
                    button: Some(SYNC),
                    serial: None,
                    known: true,
                });
            }
            "gone" => self.phone_pill.fake = None,
            "open" => self.phone_pill.forced = Some(true),
            "close" => self.phone_pill.forced = Some(false),
            "free" => self.phone_pill.forced = None,
            _ => return "phonepill fake [words]|gone|open|close|free".to_owned(),
        }
        self.phone_pill_sync();
        self.update_phone_hover();
        format!(
            "phone pill: {:?}, at {:?}",
            self.phone_pill_now().map(|s| (s.says, s.battery)),
            self.phone_pill_on_bar().map(|r| (r.x.round(), r.w.round()))
        )
    }

    /// Draw it: the long pill first (it comes out from behind), the round
    /// one on top of it.
    pub(crate) fn push_phone_pill(&self, scene: &mut Scene, rect: Rect) {
        let p = &self.phone_pill;
        let Some(shown) = p.shown.as_ref() else {
            return;
        };
        let s = self.options_scale();
        let a = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let bright = self.options_bar_is_bright();
        let ink = self.options_text_color();
        let round = Self::phone_pill_round(rect);
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Phone));
        let on_round = ptr.is_some_and(|p| p.0 >= round.x);

        // The long pill: from behind the round one, leftward.
        let out = a * ((p.open - 0.1) / 0.6).clamp(0.0, 1.0);
        let long = self.phone_pill_long(rect);
        if out > 0.004 && long.w > 2.0 {
            push_neumorph(scene, long, long.h / 2.0, bright, out);
            let wash = self.options_rest_wash();
            scene.rects.push(RectInst {
                rect: long,
                radius: long.h / 2.0,
                color: [wash[0], wash[1], wash[2], wash[3] * out],
                glass: 0.0,
                border: 0.0,
            });
            // The words at the pill's left end, the battery (a glyph of the
            // bar's icon font) and its number at its right end, the pill's
            // spare length between them (Max, 2026-10-11: "separate the
            // battery from the state, leave the air in the middle").
            let clip = Some(Rect::new(long.x + PILL_PAD_X, long.y, (long.w - 2.0 * PILL_PAD_X).max(0.0), long.h));
            let color = Some([ink[0], ink[1], ink[2], ink[3] * out]);
            let ty = long.y + (long.h - LINE_PX * s) / 2.0;
            let mut x = long.x + PILL_PAD_X;
            let mut write = |scene: &mut Scene, text: String, w: f32, family: Option<&'static str>| {
                let k = if family.is_some() { BATTERY_SIZE } else { 1.0 };
                scene.labels.push(Label {
                    text,
                    pos: (x, ty - LINE_PX * s * (k - 1.0) / 2.0),
                    max_w: 4096.0,
                    font_px: FONT_PX * s * k,
                    line_px: LINE_PX * s * k,
                    centered: false,
                    dim: false,
                    cache: true,
                    family,
                    color,
                    clip,
                });
                x += w;
            };
            // (The words' step carries the pen to where the battery begins.)
            let battery_w = p.glyph_w + BATTERY_GAP + p.level_w;
            write(scene, shown.says.clone(), (long.w - 2.0 * PILL_PAD_X - battery_w).max(0.0), TEXT_FONT);
            if let Some(level) = shown.battery {
                write(scene, battery_glyph(level).to_owned(), p.glyph_w + BATTERY_GAP, Some(crate::options::NERD));
                write(scene, format!("{level}%"), p.level_w, TEXT_FONT);
            }
            // The button: the bell's amber chip, a little stronger under
            // the pointer (the task pill's Cancel is the same chip).
            if let (Some(chip), Some(word)) = (self.phone_pill_chip(long), shown.button) {
                let lit = ptr.is_some_and(|p| chip.contains(p));
                let amber = crate::notif::AMBER;
                scene.rects.push(RectInst {
                    rect: chip,
                    radius: chip.h / 2.0,
                    color: [amber[0], amber[1], amber[2], if lit { 0.38 } else { 0.2 } * out],
                    glass: 0.0,
                    border: 0.0,
                });
                scene.labels.push(Label {
                    text: word.to_owned(),
                    pos: (chip.x + chip.w / 2.0, ty),
                    max_w: 4096.0,
                    font_px: FONT_PX * s,
                    line_px: LINE_PX * s,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: TEXT_FONT,
                    color,
                    clip: Some(chip),
                });
            }
        }

        // The round pill: every pill's ground, lifted under the pointer as
        // the bell is.
        let lift = f32::from(u8::from(on_round));
        let grown = hover_grow(round);
        let r = Rect::new(
            lerp(round.x, grown.x, lift),
            lerp(round.y, grown.y, lift),
            lerp(round.w, grown.w, lift),
            lerp(round.h, grown.h, lift),
        );
        push_neumorph(scene, r, r.h / 2.0, bright, a);
        let mut wash = lerp4(self.options_rest_wash(), self.options_hover_wash(), lift);
        wash[3] *= a;
        scene.rects.push(RectInst { rect: r, radius: r.h / 2.0, color: wash, glass: 0.0, border: 0.0 });
        // Under the pointer it says what a click does: an X (the phone is
        // ejected) — Max, 2026-10-11. (fa-mobile sits small in its box:
        // drawn a little larger than the pills' own glyphs.)
        let (glyph, k) = if on_round { (crate::options::GLYPH_CLOSE, 1.0) } else { (GLYPH_PHONE, 1.15) };
        let (font, line) = (FONT_PX * s * k, LINE_PX * s * k);
        scene.labels.push(Label {
            text: glyph.to_owned(),
            pos: (r.x + r.w / 2.0, r.y + (r.h - line) / 2.0),
            max_w: r.w + 4.0,
            font_px: font,
            line_px: line,
            centered: true,
            dim: false,
            cache: true,
            family: Some(crate::options::NERD),
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(r),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn how_long_ago_is_said_short() {
        assert_eq!(ago(20), "just now");
        assert_eq!(ago(5 * 60), "5 min ago");
        assert_eq!(ago(89 * 60), "89 min ago");
        assert_eq!(ago(2 * 3600), "2 h ago");
        assert_eq!(ago(35 * 3600), "35 h ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }
}

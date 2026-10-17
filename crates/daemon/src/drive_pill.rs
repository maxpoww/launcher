//! The DRIVE pills: on the OPTIONS bar for as long as a stick or a memory
//! card is plugged in — Max, 2026-10-17: *"build pills following the UX of
//! the phone's pills but with the new look"*.
//!
//! AT REST: a round pill per drive with what it is on it (a memory card, a
//! stick), left of the phone pill (left of the bell when there is none).
//!
//! THE POINTER ON IT: a long pill comes out from behind it, to the left, as
//! the phone's does, saying which drive it is and how full: `NIKON · 12.4 GB
//! free of 15.9 GB`; `· Ejecting…` once it is being let go.
//!
//! A CLICK on the round pill (an X under the pointer, as the phone's) ejects
//! the drive; on the long one, its folder opens.
//!
//! The quiet look (2026-10-16): the round pill is every pill's ground; the
//! long one is the pills' rest wash, nothing behind its words, the free
//! space a hairline under them in the accent.

use std::time::Instant;

use crate::animation::{self, ease_toward, lerp, lerp4};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::i18n::tr;
use crate::options::{hover_grow, push_neumorph, PillId, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-usb (the gear's drive list wears it too).
const GLYPH_STICK: &str = "\u{f287}";
/// md-sd: a memory card.
const GLYPH_CARD: &str = "\u{f0479}";
/// md-disc.
const GLYPH_DISC: &str = "\u{f019a}";

/// How many drives get a pill (more than that is a hub of them: the
/// desktop has them all).
const MOST: usize = 4;
/// The long pill: its words and a third as much again, as the phone's.
const LONGER: f32 = 4.0 / 3.0;
/// The hairline that says how full it is.
const METER_H: f32 = 2.0;

/// One drive's pill.
#[derive(Debug, Clone, Default)]
struct DrivePill {
    /// Its folder (what it is known by, what is ejected and opened).
    path: String,
    glyph: &'static str,
    /// "NIKON · 12.4 GB free of 15.9 GB".
    says: String,
    says_w: f32,
    measured: String,
    /// Used, 0..1 (`None`: not known).
    full: Option<f32>,
    /// 0 (gone) → 1 (there).
    t: f32,
    /// 0 (the round pill) → 1 (the long one out).
    open: f32,
    /// Plugged in still.
    there: bool,
    /// Being let go.
    ejecting: bool,
}

/// The pills' state.
#[derive(Debug, Default)]
pub(crate) struct DrivePills {
    pills: Vec<DrivePill>,
    /// The one whose long pill is asked out.
    want_open: Option<usize>,
    last: Option<Instant>,
    frame_pending: bool,
}

/// What a drive says: its name, and what is free on it.
fn says(name: &str, space: Option<(u64, u64)>, ejecting: bool) -> String {
    if ejecting {
        return format!("{name} · {}", tr("Ejecting…"));
    }
    match space {
        Some((total, free)) if total > 0 => crate::i18n::fill(
            tr("{0} · {1} free of {2}"),
            &[("0", &name), ("1", &crate::desktop_props::size_text(free)), ("2", &crate::desktop_props::size_text(total))],
        ),
        _ => name.to_owned(),
    }
}

/// The glyph for a volume's icon name (`mounts::volume_icon`).
fn glyph_of(icon: &str) -> &'static str {
    match icon {
        "media-flash" => GLYPH_CARD,
        "drive-optical" => GLYPH_DISC,
        _ => GLYPH_STICK,
    }
}

impl App {
    /// The sticks and cards plugged in now: their folders, names and icons.
    fn drives_now(&self) -> Vec<(String, String, &'static str)> {
        self.desktop
            .volumes
            .iter()
            .filter(|v| !v.phone && !v.closed)
            .take(MOST)
            .map(|v| (v.path.to_string_lossy().into_owned(), v.name.clone(), glyph_of(crate::mounts::volume_icon(v))))
            .collect()
    }

    /// What is plugged in changed, or what is free on it may have: the pills
    /// follow (a new one comes, a gone one goes, each says how full).
    pub(crate) fn drive_pills_sync(&mut self) {
        let now = self.drives_now();
        for p in &mut self.drive_pills.pills {
            p.there = now.iter().any(|(path, _, _)| *path == p.path);
        }
        for (path, name, glyph) in now {
            let space = crate::desktop_props::space(std::path::Path::new(&path));
            let full = space.filter(|(t, _)| *t > 0).map(|(t, f)| 1.0 - f as f32 / t as f32);
            match self.drive_pills.pills.iter_mut().find(|p| p.path == path) {
                Some(p) => {
                    p.says = says(&name, space, p.ejecting);
                    p.full = full;
                    p.glyph = glyph;
                }
                None => self.drive_pills.pills.push(DrivePill {
                    says: says(&name, space, false),
                    path,
                    glyph,
                    full,
                    there: true,
                    ..Default::default()
                }),
            }
        }
        let font_px = FONT_PX * self.options_scale();
        if let Some(r) = self.options_renderer.as_mut() {
            for p in &mut self.drive_pills.pills {
                if p.measured != p.says {
                    p.says_w = r.measure_text(&p.says, font_px, TEXT_FONT);
                    p.measured = p.says.clone();
                }
            }
        }
        self.schedule_drive_pills_frame();
        self.draw_options();
    }

    fn schedule_drive_pills_frame(&mut self) {
        self.schedule_tick(|a| (&mut a.drive_pills.frame_pending, &mut a.drive_pills.last), App::tick_drive_pills);
    }

    /// One frame of their coming, opening and going.
    fn tick_drive_pills(&mut self) {
        let now = Instant::now();
        let dt = self.drive_pills.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.drive_pills.last = Some(now);
        let want = self.drive_pills.want_open;
        let mut moving = false;
        let widths: Vec<f32> = (0..self.drive_pills.pills.len()).map(|i| self.drive_long_w(i)).collect();
        for (i, p) in self.drive_pills.pills.iter_mut().enumerate() {
            let settle = animation::settle_t(widths[i].max(1.0));
            let (t, a) = ease_toward(p.t, f32::from(u8::from(p.there)), dt, animation::MORPH_RATE, settle);
            let open_to = f32::from(u8::from(want == Some(i) && p.there));
            let (open, b) = ease_toward(p.open, open_to, dt, animation::MORPH_RATE, settle);
            (p.t, p.open) = (t, open);
            moving |= a || b;
        }
        // (Gone, and all the way out: forgotten — the one asked open keeps
        // its number.)
        if !moving {
            let before = self.drive_pills.pills.len();
            self.drive_pills.pills.retain(|p| p.there || p.t > 0.01);
            if self.drive_pills.pills.len() != before {
                self.drive_pills.want_open = None;
            }
        }
        self.sync_options_input();
        self.draw_options();
        if moving {
            self.schedule_drive_pills_frame();
        } else {
            self.drive_pills.last = None;
        }
    }

    /// Drive `i`'s long pill, fully out: its words, the pills' padding, and
    /// a third as much again.
    fn drive_long_w(&self, i: usize) -> f32 {
        let w = self.drive_pills.pills.get(i).map_or(0.0, |p| p.says_w);
        (w + 2.0 * PILL_PAD_X) * LONGER
    }

    /// How much of the bar the round pills take, left of the phone pill
    /// (what the pills that stand by the bell stop short of).
    pub(crate) fn drive_pills_reserve(&self) -> f32 {
        let ph = self.options_pill_h();
        self.drive_pills.pills.iter().map(|p| (ph + OPTION_GAP) * p.t.clamp(0.0, 1.0)).sum()
    }

    /// Where each pill is on the bar now: its round pill in the row left of
    /// `right`, and left of it as much of its long one as is out.
    pub(crate) fn drive_pill_rects(&self, right: f32, y: f32, ph: f32) -> Vec<(u8, Rect)> {
        let mut out = Vec::new();
        let mut edge = right;
        for (i, p) in self.drive_pills.pills.iter().enumerate() {
            let step = (ph + OPTION_GAP) * p.t.clamp(0.0, 1.0);
            if p.t > 0.01 {
                let long = (OPTION_GAP + self.drive_long_w(i)) * p.open;
                let r = edge - OPTION_GAP;
                out.push((i as u8, Rect::new(r - ph - long, y, ph + long, ph)));
            }
            edge -= step;
        }
        out
    }

    /// Whose long pill is out (it stands over what was there).
    pub(crate) fn drive_pill_out(&self) -> Option<u8> {
        self.drive_pills.pills.iter().position(|p| p.open > 0.05).map(|i| i as u8)
    }

    /// The pointer moved on the bar: the pill under it opens.
    pub(crate) fn update_drive_hover(&mut self) -> bool {
        let on = match self.options_hover {
            Some(PillId::Drive(i)) => Some(usize::from(i)),
            _ => None,
        };
        if on != self.drive_pills.want_open {
            self.drive_pills.want_open = on;
            // (How full it is is read as it opens.)
            if on.is_some() {
                self.drive_pills_sync();
            }
            self.schedule_drive_pills_frame();
        }
        on.is_some()
    }

    /// The pointer left the bar: the long pill goes back in.
    pub(crate) fn drive_pills_close(&mut self) {
        if self.drive_pills.want_open.take().is_some() {
            self.schedule_drive_pills_frame();
        }
    }

    /// A click on pill `i`: on the round one, the drive is ejected; on the
    /// long one, its folder opens.
    pub(crate) fn drive_pill_click(&mut self, i: u8) {
        let Some(path) = self.drive_pills.pills.get(usize::from(i)).filter(|p| p.there).map(|p| p.path.clone()) else {
            return;
        };
        let (Some(ptr), Some(rect)) = (self.options_ptr, self.drive_pill_on_bar(i)) else {
            return;
        };
        let ph = self.options_pill_h().min(rect.h);
        if ptr.0 >= rect.x + rect.w - ph {
            if let Some(p) = self.drive_pills.pills.get_mut(usize::from(i)) {
                p.ejecting = true;
            }
            self.desktop_eject(&path);
            self.drive_pills_sync();
        } else if let Some(item) = self.desktop.items.iter().position(|it| it.path == path) {
            self.drive_pills_close();
            self.desktop_activate(item);
        }
    }

    /// Pill `i`'s rectangle as the bar placed it.
    fn drive_pill_on_bar(&self, i: u8) -> Option<Rect> {
        self.drive_pill_drawn(i)
    }

    /// `debug-desktop "drivepill <open N|shut|click N|state>"`: the pills
    /// without a pointer (the nested rig has none).
    pub(crate) fn drive_pill_debug(&mut self, what: &str) -> String {
        let (verb, arg) = what.split_once(' ').unwrap_or((what, ""));
        let n = arg.trim().parse::<u8>().unwrap_or(0);
        match verb {
            "open" => {
                self.drive_pills.want_open = Some(usize::from(n));
                self.drive_pills_sync();
            }
            "shut" => self.drive_pills_close(),
            "sync" => self.drive_pills_sync(),
            _ => {}
        }
        self.drive_pills
            .pills
            .iter()
            .map(|p| format!("{} t={:.2} open={:.2}", p.says, p.t, p.open))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub(crate) fn push_drive_pill(&self, scene: &mut Scene, rect: Rect, i: u8) {
        let Some(p) = self.drive_pills.pills.get(usize::from(i)) else {
            return;
        };
        let s = self.options_scale();
        let a = ((p.t - 0.15) / 0.6).clamp(0.0, 1.0);
        let bright = self.options_bar_is_bright();
        let ink = self.options_text_color();
        let ph = self.options_pill_h().min(rect.h);
        let round = Rect::new(rect.x + rect.w - ph, rect.y, ph, ph);
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Drive(i)));
        let on_round = ptr.is_some_and(|q| q.0 >= round.x);

        // The long pill: from behind the round one, leftward — the pills'
        // rest wash, the words on it, and how full the drive is as a
        // hairline under them (the quiet language's 2 px track).
        let out = a * ((p.open - 0.1) / 0.6).clamp(0.0, 1.0);
        let long = Rect::new(rect.x, rect.y, (rect.w - ph - OPTION_GAP * p.open).max(0.0), ph);
        if out > 0.004 && long.w > 2.0 {
            push_neumorph(scene, long, ph / 2.0, bright, out);
            let on_long = ptr.is_some_and(|q| long.contains(q));
            let mut wash = lerp4(self.options_rest_wash(), self.options_hover_wash(), f32::from(u8::from(on_long)));
            wash[3] *= out;
            scene.rects.push(RectInst { rect: long, radius: ph / 2.0, color: wash, glass: 0.0, border: 0.0 });
            let inner = Rect::new(long.x + PILL_PAD_X, long.y, (long.w - 2.0 * PILL_PAD_X).max(0.0), long.h);
            let ty = long.y + (long.h - LINE_PX * s) / 2.0;
            scene.labels.push(Label {
                text: p.says.clone(),
                pos: (inner.x + ((inner.w - p.says_w) / 2.0).max(0.0), ty),
                max_w: 4096.0,
                font_px: FONT_PX * s,
                line_px: LINE_PX * s,
                centered: false,
                dim: false,
                cache: true,
                family: TEXT_FONT,
                color: Some([ink[0], ink[1], ink[2], ink[3] * out]),
                clip: Some(inner),
            });
            if let (Some(full), false) = (p.full, p.ejecting) {
                let track = Rect::new(inner.x, long.y + long.h - 5.0 * s, inner.w, METER_H * s);
                scene.rects.push(RectInst {
                    rect: track,
                    radius: track.h / 2.0,
                    color: [ink[0], ink[1], ink[2], 0.12 * out],
                    glass: 0.0,
                    border: 0.0,
                });
                let accent = crate::palette::accent_srgb();
                scene.rects.push(RectInst {
                    rect: Rect::new(track.x, track.y, track.w * full.clamp(0.0, 1.0), track.h),
                    radius: track.h / 2.0,
                    color: [accent[0], accent[1], accent[2], 0.9 * out],
                    glass: 0.0,
                    border: 0.0,
                });
            }
        }

        // The round pill: every pill's ground, lifted under the pointer.
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
        // Under the pointer it says what a click does: an X (it is ejected).
        let glyph = if on_round { crate::options::GLYPH_CLOSE } else { p.glyph };
        let (font, line) = (FONT_PX * s * 1.1, LINE_PX * s * 1.1);
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
    use super::*;

    #[test]
    fn a_drive_says_its_name_and_what_is_free() {
        assert_eq!(says("NIKON", Some((16 << 30, 12 << 30)), false), "NIKON · 12.0 GB free of 16.0 GB");
        assert_eq!(says("NIKON", None, false), "NIKON");
        assert_eq!(says("NIKON", Some((1, 1)), true), "NIKON · Ejecting…");
        assert_eq!(glyph_of("media-flash"), GLYPH_CARD);
        assert_eq!(glyph_of("drive-removable-media-usb"), GLYPH_STICK);
    }
}

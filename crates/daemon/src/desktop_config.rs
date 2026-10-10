//! A phone's CONFIGURE box (Max, 2026-10-10, settled on
//! `~/phone-configure-mockup`): the bigger box behind the "Configure" row of
//! a phone's menu, where what this computer does with that phone is set and
//! then APPLIED or CANCELLED.
//!
//! Three parts, from the top: its FOLDER here (named by its owner — there may
//! be two phones of one model), SYNC OVER WI-FI (on or off, what of the
//! phone, how often, only while it charges), and CLEAN THE PHONE (its files
//! moved off it into that folder; asked once more before it starts, because
//! it deletes from the phone).
//!
//! One function (`build`) places every part and says where it can be
//! clicked; the draw and the pointer both walk what it returns. The box is
//! drawn where the menu and the Properties box are (the surface above the
//! windows). Nothing changes until Apply — except Clean, which says what it
//! will do and is then done with the settings as they stand in the box.

use std::collections::HashMap;

use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_BOLD};
use crate::desktop_menu::MenuPaint;
use crate::phones::{Phone, KINDS};

pub(crate) const WIDTH: f32 = 520.0;
const PAD: f32 = 18.0;
const FONT_PX: f32 = 13.0;
const LINE_PX: f32 = 17.0;
const TITLE_PX: f32 = 16.0;
const TITLE_LINE: f32 = 21.0;
const HEAD_PX: f32 = 14.0;
const ROW_H: f32 = 27.0;
const SAY_LINE: f32 = 17.0;
/// Where a line's answer starts.
const VALUE_X: f32 = 150.0;
const DIM: f32 = 0.58;
const LINE_INK: f32 = 0.12;
/// The switch that is on, the box that is ticked, Apply.
const ACCENT: [f32; 3] = [0.47, 0.78, 0.745];
/// What takes files off the phone.
const DANGER: [f32; 3] = [1.0, 0.43, 0.43];
/// How many letters of the small text fit a line of the box (it has no
/// renderer to measure with: a wrap by count, on the safe side).
const SAY_CHARS: usize = 64;

/// What of the box can be clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    Name,
    Sync,
    Kind(usize),
    Every,
    Charging,
    After,
    Clean,
    Really,
    NotNow,
    Cancel,
    Apply,
}

/// A Configure box that is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Config {
    /// The phone's place on the desktop (its volume's path), its serial and
    /// the name it gives itself.
    pub path: String,
    pub serial: String,
    pub model: String,
    /// The line under the title (Android, battery, storage), once it said.
    pub sub: String,
    /// What is being set — a copy: the phone's own until Apply.
    pub phone: Phone,
    /// How much the phone holds of each kind, once it said.
    pub sizes: HashMap<&'static str, u64>,
    /// The name is being typed; `all`: it is selected whole (the first key
    /// replaces it).
    pub editing: bool,
    pub all: bool,
    /// Clean was asked for: it is being asked once more.
    pub confirm: bool,
    pub rect: Rect,
    /// Its entrance, 0..1.
    pub t: f32,
    pub hover: Option<Hit>,
}

/// Everything the box is made of, placed.
struct Built {
    rects: Vec<RectInst>,
    labels: Vec<Label>,
    hits: Vec<(Hit, Rect)>,
    height: f32,
}

/// `text` cut into lines of no more than `SAY_CHARS`, at its spaces.
fn wrapped(text: &str) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("there is always a line");
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > SAY_CHARS {
            lines.push(word.to_owned());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

impl Config {
    /// The box for `phone`, by the pointer at `at` on a `w`×`h` surface (or
    /// flipped to stay on it).
    pub fn open(path: &str, serial: &str, model: &str, phone: Phone, at: (f32, f32), w: f32, h: f32) -> Self {
        let mut config = Self {
            path: path.to_owned(),
            serial: serial.to_owned(),
            model: model.to_owned(),
            sub: String::new(),
            phone,
            sizes: HashMap::new(),
            editing: false,
            all: false,
            confirm: false,
            rect: Rect::new(0.0, 0.0, WIDTH, 0.0),
            t: 0.0,
            hover: None,
        };
        let height = config.build([1.0; 4], [0.0; 4]).height;
        let x = if at.0 + 8.0 + WIDTH <= w { at.0 + 8.0 } else { (at.0 - 8.0 - WIDTH).max(0.0) };
        let y = if at.1 + height <= h { at.1 } else { (h - height).max(0.0) };
        config.rect = Rect::new(x, y, WIDTH, height);
        config
    }

    /// What it holds changed its height (the question before a clean): the
    /// box takes it, from the same top, kept on a surface `h` tall.
    pub fn fit(&mut self, h: f32) {
        self.rect.h = self.build([1.0; 4], [0.0; 4]).height;
        self.rect.y = self.rect.y.min(h - self.rect.h).max(0.0);
    }

    /// What is under `pos`.
    pub fn hit(&self, pos: (f32, f32)) -> Option<Hit> {
        self.build([1.0; 4], [0.0; 4]).hits.into_iter().find(|(_, r)| r.contains(pos)).map(|(hit, _)| hit)
    }

    /// The phone's folder as the box writes it.
    fn folder_text(&self) -> String {
        format!("~/Phones/{}", crate::phones::folder_name(&self.phone.name))
    }

    /// How much a clean would free, of what is ticked.
    fn would_free(&self) -> Option<u64> {
        (!self.sizes.is_empty()).then(|| self.phone.its_kinds().iter().filter_map(|k| self.sizes.get(k.key)).sum())
    }

    /// Place everything. `ink` is the text's colour, `wash` a control's
    /// ground.
    fn build(&self, ink: [f32; 4], wash: [f32; 4]) -> Built {
        let mut b = Built { rects: Vec::new(), labels: Vec::new(), hits: Vec::new(), height: 0.0 };
        let (x, w) = (self.rect.x + PAD, self.rect.w - 2.0 * PAD);
        let right = x + w;
        let mut y = self.rect.y + PAD;
        let at = |a: f32| [ink[0], ink[1], ink[2], ink[3] * a];
        let tint = |c: [f32; 3], a: f32| [c[0], c[1], c[2], a * ink[3]];
        let text = |s: &str, pos: (f32, f32), max_w: f32, px: f32, line: f32, bold: bool, color: [f32; 4]| Label {
            text: s.to_owned(),
            pos,
            max_w,
            font_px: px,
            line_px: line,
            centered: false,
            dim: false,
            cache: false,
            clip: Some(Rect::new(pos.0, pos.1 - 2.0, max_w, line + 4.0)),
            family: bold.then_some(FONT_BOLD),
            color: Some(color),
        };
        let centred = |s: &str, r: Rect, color: [f32; 4]| Label {
            text: s.to_owned(),
            pos: (r.x + r.w / 2.0, r.y + (r.h - LINE_PX) / 2.0),
            max_w: r.w,
            font_px: FONT_PX,
            line_px: LINE_PX,
            centered: true,
            dim: false,
            cache: false,
            clip: Some(r),
            family: None,
            color: Some(color),
        };
        let lit = |hit: Hit| self.hover == Some(hit);
        let rule = |b: &mut Built, y: &mut f32| {
            *y += 12.0;
            b.rects.push(RectInst { rect: Rect::new(self.rect.x, *y, self.rect.w, 1.0), radius: 0.0, color: at(LINE_INK), glass: 0.0, border: 0.0 });
            *y += 13.0;
        };
        // A switch at the right end of the line whose top is `y`.
        let switch = |b: &mut Built, y: f32, on: bool, hit: Hit, live: bool| {
            let r = Rect::new(right - 36.0, y + (LINE_PX - 20.0) / 2.0, 36.0, 20.0);
            let fade = if live { 1.0 } else { 0.4 };
            let ground = if on { tint(ACCENT, 0.9 * fade) } else { at(if lit(hit) { 0.26 } else { 0.18 } * fade) };
            b.rects.push(RectInst { rect: r, radius: 10.0, color: ground, glass: 0.0, border: 0.0 });
            let knob = if on { r.x + r.w - 17.0 } else { r.x + 3.0 };
            b.rects.push(RectInst { rect: Rect::new(knob, r.y + 3.0, 14.0, 14.0), radius: 7.0, color: [1.0, 1.0, 1.0, 0.95 * ink[3] * fade], glass: 0.0, border: 0.0 });
            if live {
                b.hits.push((hit, Rect::new(r.x - 6.0, r.y - 4.0, r.w + 12.0, r.h + 8.0)));
            }
        };
        // A value that is a choice: a pill, a click takes the next one.
        let choice = |b: &mut Built, y: f32, value: &str, hit: Hit, live: bool| {
            let r = Rect::new(x + VALUE_X, y - 4.0, w - VALUE_X, 25.0);
            let fade = if live { 1.0 } else { 0.4 };
            b.rects.push(RectInst { rect: r, radius: 7.0, color: [wash[0], wash[1], wash[2], wash[3] * if lit(hit) { 1.7 } else { 1.0 } * fade], glass: 0.0, border: 0.0 });
            b.labels.push(text(value, (r.x + 10.0, y), r.w - 34.0, FONT_PX, LINE_PX, false, at(0.92 * fade)));
            b.labels.push(text("›", (r.x + r.w - 18.0, y), 12.0, FONT_PX, LINE_PX, false, at(DIM * fade)));
            if live {
                b.hits.push((hit, r));
            }
        };
        // An amber chip (a red one for what deletes), like the task pill's.
        let chip = |b: &mut Built, r: Rect, word: &str, hit: Hit, danger: bool| {
            let c = if danger { DANGER } else { [1.0, 0.745, 0.596] };
            b.rects.push(RectInst { rect: r, radius: r.h / 2.0, color: tint(c, if lit(hit) { 0.42 } else { 0.22 }), glass: 0.0, border: 0.0 });
            b.labels.push(centred(word, r, at(1.0)));
            b.hits.push((hit, r));
        };

        // ── The phone ──
        b.labels.push(text(&self.phone.name, (x, y), w, TITLE_PX, TITLE_LINE, true, at(1.0)));
        y += TITLE_LINE + 2.0;
        let sub = if self.sub.is_empty() { self.model.as_str() } else { self.sub.as_str() };
        b.labels.push(text(sub, (x, y), w, FONT_PX, LINE_PX, false, at(DIM)));
        y += LINE_PX;

        // ── Its folder here ──
        rule(&mut b, &mut y);
        b.labels.push(text("Its folder on this computer", (x, y), w, HEAD_PX, LINE_PX, true, at(1.0)));
        y += LINE_PX + 4.0;
        for line in wrapped("Everything from this phone is kept in one folder, so it is always in the same place.") {
            b.labels.push(text(&line, (x, y), w, FONT_PX, SAY_LINE, false, at(DIM)));
            y += SAY_LINE;
        }
        y += 8.0;
        b.labels.push(text("Name", (x, y), VALUE_X - 8.0, FONT_PX, LINE_PX, false, at(DIM)));
        // The name, typed by its owner: a field.
        let field = Rect::new(x + VALUE_X, y - 4.0, w - VALUE_X, 25.0);
        let field_wash = if self.editing { 2.2 } else if lit(Hit::Name) { 1.7 } else { 1.0 };
        b.rects.push(RectInst { rect: field, radius: 7.0, color: [wash[0], wash[1], wash[2], wash[3] * field_wash], glass: 0.0, border: 0.0 });
        if self.editing && self.all && !self.phone.name.is_empty() {
            // Selected whole: the first key replaces it.
            let sel_w = (self.phone.name.chars().count() as f32 * FONT_PX * 0.56 + 8.0).min(field.w - 12.0);
            b.rects.push(RectInst { rect: Rect::new(field.x + 6.0, field.y + 3.0, sel_w, field.h - 6.0), radius: 4.0, color: tint(ACCENT, 0.35), glass: 0.0, border: 0.0 });
        }
        let typed = if self.editing && !self.all { format!("{}│", self.phone.name) } else { self.phone.name.clone() };
        b.labels.push(text(&typed, (field.x + 10.0, y), field.w - 20.0, FONT_PX, LINE_PX, false, at(0.95)));
        b.hits.push((Hit::Name, field));
        y += ROW_H;
        b.labels.push(text("Folder", (x, y), VALUE_X - 8.0, FONT_PX, LINE_PX, false, at(DIM)));
        b.labels.push(text(&self.folder_text(), (x + VALUE_X + 10.0, y), w - VALUE_X - 10.0, FONT_PX, LINE_PX, false, at(0.92)));
        y += LINE_PX;

        // ── Sync over Wi-Fi ──
        rule(&mut b, &mut y);
        b.labels.push(text("Sync over Wi-Fi", (x, y), w - 50.0, HEAD_PX, LINE_PX, true, at(1.0)));
        switch(&mut b, y, self.phone.sync, Hit::Sync, true);
        y += LINE_PX + 4.0;
        for line in wrapped("When the phone is on the same Wi-Fi as this computer, new files on it are copied to its folder here, without the cable. It has to be plugged in once to allow it, and again after the phone restarts.") {
            b.labels.push(text(&line, (x, y), w, FONT_PX, SAY_LINE, false, at(DIM)));
            y += SAY_LINE;
        }
        y += 8.0;
        // What of the phone: two columns. (Clean goes by these too, so they
        // can be ticked whether or not it is synced.)
        let col_w = (w - 16.0) / 2.0;
        for (i, kind) in KINDS.iter().enumerate() {
            let (cx, cy) = (x + (i % 2) as f32 * (col_w + 16.0), y + (i / 2) as f32 * ROW_H);
            let on = self.phone.kinds.iter().any(|k| k == kind.key);
            let tick = Rect::new(cx, cy + (LINE_PX - 16.0) / 2.0, 16.0, 16.0);
            let ground = if on { tint(ACCENT, 0.9) } else { at(if lit(Hit::Kind(i)) { 0.26 } else { 0.16 }) };
            b.rects.push(RectInst { rect: tick, radius: 4.0, color: ground, glass: 0.0, border: 0.0 });
            if on {
                b.labels.push(centred("✓", tick, [0.05, 0.08, 0.08, ink[3]]));
            }
            b.labels.push(text(kind.label, (cx + 24.0, cy), col_w - 100.0, FONT_PX, LINE_PX, false, at(0.92)));
            let size = self.sizes.get(kind.key).map(|s| crate::desktop_props::size_text(*s)).unwrap_or_default();
            b.labels.push(text(&size, (cx + col_w - 70.0, cy), 70.0, FONT_PX, LINE_PX, false, at(DIM)));
            b.hits.push((Hit::Kind(i), Rect::new(cx - 4.0, cy - 4.0, col_w, ROW_H - 2.0)));
        }
        y += KINDS.len().div_ceil(2) as f32 * ROW_H + 4.0;
        let live = self.phone.sync;
        let fade = if live { 1.0 } else { 0.4 };
        b.labels.push(text("How often", (x, y), VALUE_X - 8.0, FONT_PX, LINE_PX, false, at(DIM * fade)));
        choice(&mut b, y, self.phone.every.label(), Hit::Every, live);
        y += ROW_H + 2.0;
        b.labels.push(text("Only while charging", (x, y), w - 50.0, FONT_PX, LINE_PX, false, at(DIM * fade)));
        switch(&mut b, y, self.phone.charging_only, Hit::Charging, live);
        y += ROW_H;
        let state = match (self.phone.sync, &self.phone.addr, self.phone.last_sync) {
            (false, ..) => "Off.".to_owned(),
            (true, None, _) => "Not allowed over Wi-Fi yet: Apply does it, with the phone plugged in.".to_owned(),
            (true, Some(addr), None) => format!("Allowed at {addr}. Not synced yet."),
            (true, Some(_), Some(at)) => {
                let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(at);
                format!("Last sync: {}.", crate::desktop_props::date_text(Some(when)).unwrap_or_default())
            }
        };
        b.labels.push(text(&state, (x, y), w, FONT_PX, LINE_PX, false, at(DIM)));
        y += LINE_PX;

        // ── Clean the phone ──
        rule(&mut b, &mut y);
        b.labels.push(text("Clean the phone", (x, y), w - 130.0, HEAD_PX, LINE_PX, true, at(1.0)));
        if !self.confirm {
            chip(&mut b, Rect::new(right - 112.0, y - 3.0, 112.0, 23.0), "Clean now…", Hit::Clean, true);
        }
        y += LINE_PX + 4.0;
        for line in wrapped("Moves your files off the phone into its folder here and frees the space on the phone: what is ticked above. Apps, their data and the system are never touched.") {
            b.labels.push(text(&line, (x, y), w, FONT_PX, SAY_LINE, false, at(DIM)));
            y += SAY_LINE;
        }
        y += 8.0;
        b.labels.push(text("Would free", (x, y), VALUE_X - 8.0, FONT_PX, LINE_PX, false, at(DIM)));
        let free = match self.would_free() {
            Some(bytes) => format!("About {} on the phone", crate::desktop_props::size_text(bytes)),
            None => "Asking the phone…".to_owned(),
        };
        b.labels.push(text(&free, (x + VALUE_X + 10.0, y), w - VALUE_X - 10.0, FONT_PX, LINE_PX, false, at(0.92)));
        y += ROW_H;
        b.labels.push(text("After a sync", (x, y), VALUE_X - 8.0, FONT_PX, LINE_PX, false, at(DIM)));
        let after = if self.phone.remove_after_sync { "Remove what was copied (keep it clean)" } else { "Leave the files on the phone" };
        choice(&mut b, y, after, Hit::After, true);
        y += ROW_H;
        if self.confirm {
            // Asked once more: it deletes from the phone.
            y += 4.0;
            let top = y;
            let said = format!(
                "This moves {} off the phone into {}. Each file is deleted from the phone only after its copy here has been checked.",
                self.would_free().map_or_else(|| "your files".to_owned(), |b| format!("about {}", crate::desktop_props::size_text(b))),
                self.folder_text()
            );
            let lines = wrapped(&said);
            let block_h = 10.0 + lines.len() as f32 * SAY_LINE + 8.0 + 23.0 + 10.0;
            b.rects.push(RectInst { rect: Rect::new(x - 6.0, top, w + 12.0, block_h), radius: 8.0, color: tint(DANGER, 0.10), glass: 0.0, border: 0.0 });
            y += 10.0;
            for line in lines {
                b.labels.push(text(&line, (x + 4.0, y), w - 8.0, FONT_PX, SAY_LINE, false, at(0.92)));
                y += SAY_LINE;
            }
            y += 8.0;
            chip(&mut b, Rect::new(x + 4.0, y, 190.0, 23.0), "Move them off the phone", Hit::Really, true);
            chip(&mut b, Rect::new(x + 204.0, y, 84.0, 23.0), "Not now", Hit::NotNow, false);
            y += 23.0 + 10.0;
        }

        // ── Apply or cancel ──
        rule(&mut b, &mut y);
        let (apply, cancel) = (Rect::new(right - 92.0, y, 92.0, 27.0), Rect::new(right - 92.0 - 10.0 - 92.0, y, 92.0, 27.0));
        b.rects.push(RectInst { rect: cancel, radius: 13.5, color: [wash[0], wash[1], wash[2], wash[3] * if lit(Hit::Cancel) { 1.8 } else { 1.0 }], glass: 0.0, border: 0.0 });
        b.labels.push(centred("Cancel", cancel, at(0.95)));
        b.hits.push((Hit::Cancel, cancel));
        b.rects.push(RectInst { rect: apply, radius: 13.5, color: tint(ACCENT, if lit(Hit::Apply) { 0.55 } else { 0.32 }), glass: 0.0, border: 0.0 });
        b.labels.push(centred("Apply", apply, at(1.0)));
        b.hits.push((Hit::Apply, apply));
        y += 27.0 + PAD;
        b.height = y - self.rect.y;
        b
    }

    /// Draw the box over everything in `scene`, fading in.
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint) {
        let t = self.t.clamp(0.0, 1.0);
        let ink = [paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * t];
        let built = self.build(ink, [paint.wash[0], paint.wash[1], paint.wash[2], paint.wash[3] * t]);
        let panel = Rect::new(self.rect.x, self.rect.y + (1.0 - t) * -4.0, self.rect.w, self.rect.h);
        let mut grid = GridContent { clip: panel, ..Default::default() };
        grid.rects.push(RectInst {
            rect: panel,
            radius: paint.radius,
            color: [paint.fill[0], paint.fill[1], paint.fill[2], paint.fill[3] * t],
            glass: 0.0,
            border: 0.0,
        });
        grid.rects.extend(built.rects);
        grid.labels.extend(built.labels);
        scene.grids.push(grid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_box() -> Config {
        Config::open("/run/x/phone", "3A301FDJG000UW", "Pixel 8 Pro", Phone::new("Pixel 8 Pro"), (100.0, 100.0), 1600.0, 1000.0)
    }

    #[test]
    fn every_part_of_the_box_can_be_reached() {
        let config = a_box();
        let built = config.build([1.0; 4], [1.0; 4]);
        let hits: Vec<Hit> = built.hits.iter().map(|(h, _)| *h).collect();
        // Sync is off: its two settings are not for clicking.
        assert_eq!(
            hits,
            vec![
                Hit::Name,
                Hit::Sync,
                Hit::Kind(0),
                Hit::Kind(1),
                Hit::Kind(2),
                Hit::Kind(3),
                Hit::Kind(4),
                Hit::Kind(5),
                Hit::Clean,
                Hit::After,
                Hit::Cancel,
                Hit::Apply,
            ]
        );
        // Everything is inside the box, and each is found under its middle.
        for (hit, r) in &built.hits {
            assert!(r.x >= config.rect.x && r.x + r.w <= config.rect.x + config.rect.w, "{hit:?} is in the box");
            assert!(r.y + r.h <= config.rect.y + config.rect.h, "{hit:?} is in the box");
            assert_eq!(config.hit((r.x + r.w / 2.0, r.y + r.h / 2.0)), Some(*hit));
        }
        assert_eq!(config.hit((config.rect.x + 4.0, config.rect.y + 4.0)), None);
    }

    #[test]
    fn sync_on_and_the_question_before_a_clean_add_their_parts() {
        let mut config = a_box();
        let rest = config.rect.h;
        config.phone.sync = true;
        let hits: Vec<Hit> = config.build([1.0; 4], [1.0; 4]).hits.iter().map(|(h, _)| *h).collect();
        assert!(hits.contains(&Hit::Every) && hits.contains(&Hit::Charging));
        // Asked: the chip gives way to the question, and the box grows.
        config.confirm = true;
        config.fit(1000.0);
        let hits: Vec<Hit> = config.build([1.0; 4], [1.0; 4]).hits.iter().map(|(h, _)| *h).collect();
        assert!(!hits.contains(&Hit::Clean) && hits.contains(&Hit::Really) && hits.contains(&Hit::NotNow));
        assert!(config.rect.h > rest);
        // What a clean would free is what is ticked.
        config.sizes = HashMap::from([("photos", 1000), ("music", 10), ("whatsapp", 500)]);
        assert_eq!(config.would_free(), Some(1010));
        assert_eq!(config.folder_text(), "~/Phones/Pixel 8 Pro");
        assert!(wrapped(&"word ".repeat(60)).iter().all(|l| l.chars().count() <= SAY_CHARS));
    }
}

// ── The box on the desktop: opening it, the pointer, the keys, what it does ──

use smithay_client_toolkit::reexports::client::protocol::wl_pointer;
use smithay_client_toolkit::reexports::client::WEnum;
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{info, warn};

use crate::phones::Phones;
use crate::App;

/// Seconds since the epoch, now.
fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How often the phones that are synced over Wi-Fi are looked for.
pub(crate) const SYNC_TICK: std::time::Duration = std::time::Duration::from_secs(60);
/// A phone that has just come onto the Wi-Fi is synced at once — unless it
/// was synced less than this ago (it left and came back).
const SYNC_AGAIN: u64 = 300;

/// What a look for a synced phone found.
struct Probe {
    there: bool,
    /// How to reach it, if it is to be synced now.
    target: Option<String>,
    /// Where it answers over Wi-Fi, if that was (re)allowed just now.
    addr: Option<String>,
}

impl App {
    /// Open the Configure box of the phone that is desktop item `i`.
    pub(crate) fn desktop_open_config(&mut self, i: usize, at: (f32, f32)) {
        let Some(path) = self.desktop.items.get(i).map(|it| it.path.clone()) else {
            return;
        };
        let Some(v) = self.desktop.volumes.iter().find(|v| v.phone && v.path.as_os_str() == path.as_str()) else {
            return;
        };
        let (model, serial) = (v.name.clone(), crate::desktop::phone_serial_of(v).unwrap_or_default());
        let phone = Phones::load().of(&serial, &model);
        let (w, h) = self.desktop_size;
        info!("desktop: configuring {model} ({serial})");
        self.desktop.selected.clear();
        self.desktop.config = Some(Config::open(&path, &serial, &model, phone, at, w as f32, h as f32));
        self.request_desktop_draw();
        // What the phone says of itself, for the line under its name and
        // the sizes beside what can be ticked.
        if serial.is_empty() {
            return;
        }
        self.desktop_off_loop(
            move || {
                if !crate::desktop_phone::ready(&serial) {
                    return (String::new(), HashMap::new());
                }
                let facts = crate::desktop_phone::facts(&serial);
                let fact = |key: &str| facts.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
                let sub: Vec<String> = [
                    fact("Android").map(|v| format!("Android {v}")),
                    fact("Battery"),
                    fact("Storage"),
                ]
                .into_iter()
                .flatten()
                .collect();
                (sub.join("  ·  "), crate::desktop_phone::sizes(&serial))
            },
            move |app, (sub, sizes)| {
                if let Some(config) = app.desktop.config.as_mut().filter(|c| c.path == path) {
                    config.sub = sub;
                    config.sizes = sizes;
                    app.request_desktop_draw();
                }
            },
        );
    }

    /// The box driven without a pointer (`debug-desktop "config …"`).
    pub(crate) fn desktop_config_debug(&mut self, what: &str) -> String {
        if what == "syncnow" {
            // Every synced phone, now.
            let all: Vec<String> = Phones::load().by_serial.into_iter().filter(|(_, p)| p.sync).map(|(s, _)| s).collect();
            self.desktop.sync_now.extend(all.iter().cloned());
            self.phone_sync_tick();
            return format!("syncing now: {all:?}");
        }
        if let Ok(i) = what.parse::<usize>() {
            self.desktop_open_config(i, (240.0, 60.0));
        } else if let Some(name) = what.strip_prefix("name ") {
            if let Some(config) = self.desktop.config.as_mut() {
                config.phone.name = crate::phones::folder_name(name);
                self.request_desktop_draw();
            }
        } else {
            let hit = match what.split_once(' ') {
                Some(("kind", i)) => i.parse().ok().map(Hit::Kind),
                _ => match what {
                    "sync" => Some(Hit::Sync),
                    "every" => Some(Hit::Every),
                    "charging" => Some(Hit::Charging),
                    "after" => Some(Hit::After),
                    "clean" => Some(Hit::Clean),
                    "really" => Some(Hit::Really),
                    "notnow" => Some(Hit::NotNow),
                    "apply" => Some(Hit::Apply),
                    "cancel" => Some(Hit::Cancel),
                    "edit" => Some(Hit::Name),
                    _ => None,
                },
            };
            match hit {
                Some(hit) => self.desktop_config_act(hit),
                None => return "config <n>|sync|kind <i>|every|charging|after|clean|really|notnow|apply|cancel|name <text>".to_owned(),
            }
        }
        match self.desktop.config.as_ref() {
            Some(c) => format!("config: {:?}, confirm {}, sizes {}", c.phone, c.confirm, c.sizes.len()),
            None => "no Configure box is up".to_owned(),
        }
    }

    /// The pointer while the box is up: it is the box's, everywhere (a click
    /// off it does nothing — it is closed by Apply or Cancel).
    pub(crate) fn desktop_config_pointer(&mut self, event: &wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter { serial, surface_x, surface_y, .. } => {
                self.enter_serial = *serial;
                self.cursor_now = None;
                self.desktop_config_motion((*surface_x as f32, *surface_y as f32));
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                self.desktop_config_motion((*surface_x as f32, *surface_y as f32));
            }
            wl_pointer::Event::Leave { .. } => {
                self.desktop.ptr = None;
                if let Some(config) = self.desktop.config.as_mut() {
                    if config.hover.take().is_some() {
                        self.request_desktop_draw();
                    }
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. }
                if *button == crate::BTN_LEFT =>
            {
                let hit = self.desktop.ptr.and_then(|p| self.desktop.config.as_ref()?.hit(p));
                // A press anywhere but the name settles the name.
                if hit != Some(Hit::Name) {
                    self.desktop_config_edit(false);
                }
                if let Some(hit) = hit {
                    self.desktop_config_act(hit);
                }
            }
            _ => {}
        }
    }

    fn desktop_config_motion(&mut self, pos: (f32, f32)) {
        self.desktop.ptr = Some(pos);
        if let Some(config) = self.desktop.config.as_mut() {
            let hover = config.hit(pos);
            if hover != config.hover {
                config.hover = hover;
                self.request_desktop_draw();
            }
        }
    }

    /// A click on a part of the box.
    fn desktop_config_act(&mut self, hit: Hit) {
        let h = self.desktop_size.1 as f32;
        let Some(config) = self.desktop.config.as_mut() else {
            return;
        };
        match hit {
            Hit::Name => {
                self.desktop_config_edit(true);
                return;
            }
            Hit::Sync => config.phone.sync = !config.phone.sync,
            Hit::Kind(i) => {
                if let Some(kind) = KINDS.get(i) {
                    match config.phone.kinds.iter().position(|k| k == kind.key) {
                        Some(at) => {
                            config.phone.kinds.remove(at);
                        }
                        None => config.phone.kinds.push(kind.key.to_owned()),
                    }
                }
            }
            Hit::Every => config.phone.every = config.phone.every.next(),
            Hit::Charging => config.phone.charging_only = !config.phone.charging_only,
            Hit::After => config.phone.remove_after_sync = !config.phone.remove_after_sync,
            Hit::Clean => config.confirm = true,
            Hit::NotNow => config.confirm = false,
            Hit::Really => {
                self.desktop_config_clean();
                return;
            }
            Hit::Cancel => {
                self.desktop_config_edit(false);
                self.desktop.config = None;
                self.request_desktop_draw();
                return;
            }
            Hit::Apply => {
                self.desktop_config_apply();
                return;
            }
        }
        config.fit(h);
        self.request_desktop_draw();
    }

    /// Whether the name is being typed (every key is its then).
    pub(crate) fn desktop_config_typing(&self) -> bool {
        self.desktop.config.as_ref().is_some_and(|c| c.editing)
    }

    /// Start or end typing the name: the desktop takes the keyboard for
    /// exactly that long, as it does for an icon's name.
    pub(crate) fn desktop_config_edit(&mut self, on: bool) {
        let Some(config) = self.desktop.config.as_mut() else {
            return;
        };
        if config.editing == on {
            return;
        }
        config.editing = on;
        config.all = on;
        if !on {
            config.phone.name = crate::phones::folder_name(&config.phone.name);
        }
        if let Some(layer) = self.desktop_layer.as_ref() {
            if on {
                crate::surface::set_interactive(layer, true);
                let _ = self.conn.flush();
                self.cancel_keyboard_handback(crate::KbSurface::Desktop);
            } else if self.desktop.keys {
                crate::surface::set_on_demand(layer);
                let _ = self.conn.flush();
            } else {
                // Armed before the release: the compositor's `leave` completes it.
                self.begin_keyboard_handback(crate::KbSurface::Desktop, None);
                if let Some(layer) = self.desktop_layer.as_ref() {
                    crate::surface::set_interactive(layer, false);
                }
                let _ = self.conn.flush();
            }
        }
        self.request_desktop_draw();
    }

    /// A key while the name is being typed.
    pub(crate) fn desktop_config_key(&mut self, keysym: Keysym, utf8: Option<&str>) {
        let ctrl = self.modifiers.ctrl;
        let Some(config) = self.desktop.config.as_mut() else {
            return;
        };
        match keysym {
            Keysym::Return | Keysym::KP_Enter | Keysym::Escape | Keysym::Tab => {
                self.desktop_config_edit(false);
                return;
            }
            Keysym::BackSpace => {
                if config.all {
                    config.phone.name.clear();
                    config.all = false;
                } else {
                    config.phone.name.pop();
                }
            }
            _ => {
                let Some(s) = utf8.filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) else {
                    return;
                };
                if ctrl || s.contains('/') {
                    return;
                }
                if config.all {
                    config.phone.name.clear();
                    config.all = false;
                }
                if config.phone.name.chars().count() < 48 {
                    config.phone.name.push_str(s);
                }
            }
        }
        self.request_desktop_draw();
    }

    /// Take what the box holds as the phone's settings from now on; the box
    /// goes. Its folder is made, and with sync on the phone is allowed over
    /// Wi-Fi (it must be on its cable for that).
    fn desktop_config_apply(&mut self) -> Option<(String, crate::phones::Phone)> {
        self.desktop_config_edit(false);
        let config = self.desktop.config.take()?;
        self.request_desktop_draw();
        let mut phone = config.phone.clone();
        phone.name = crate::phones::folder_name(&phone.name);
        if config.serial.is_empty() {
            // Nothing to know it by: it cannot be remembered.
            let said = format!("{}: {}", config.model, crate::i18n::tr("turn on USB debugging on the phone to configure it."));
            std::thread::spawn(move || crate::desktop_send_notify(&said));
            return None;
        }
        let mut phones = Phones::load();
        // A change of name is a change of folder: the old one goes with it.
        // (A phone never configured has the folder its imports made, under
        // the name it gave itself: that one comes along too — it stayed
        // behind, and the first sync copied every photo a second time.)
        let before = phones
            .by_serial
            .get(&config.serial)
            .map(|p| p.folder())
            .or_else(|| Some(crate::phones::phones_dir().join(crate::phones::folder_name(&config.model))));
        // (What the box does not set is as it is NOW, not as it was when the
        // box opened: a sync may have ended meanwhile.)
        if let Some(stored) = phones.by_serial.get(&config.serial) {
            phone.addr = stored.addr.clone();
            phone.last_sync = stored.last_sync;
        }
        phones.by_serial.insert(config.serial.clone(), phone.clone());
        phones.save();
        info!("desktop: {} is configured: {phone:?}", config.serial);
        let (serial, model, set) = (config.serial.clone(), config.model.clone(), phone.clone());
        self.desktop_off_loop(
            move || {
                let folder = set.folder();
                if let Some(old) = before.filter(|old| *old != folder && old.is_dir() && !folder.exists()) {
                    if let Some(parent) = folder.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::rename(&old, &folder) {
                        warn!("phone: {} could not become {}: {e}", old.display(), folder.display());
                    }
                }
                let _ = std::fs::create_dir_all(&folder);
                crate::desktop_phone::adopt_pictures(&model, &folder);
                // Over Wi-Fi from now on (again at each Apply: the phone
                // forgets it when it restarts, and its address can change).
                let allowed = (set.sync && crate::desktop_phone::ready(&serial))
                    .then(|| crate::desktop_phone::wifi_allow(&serial))
                    .flatten();
                if set.sync && allowed.is_none() && set.addr.is_none() {
                    crate::desktop_send_notify_open(
                        &set.name,
                        "phone",
                        crate::i18n::tr("It could not be allowed over Wi-Fi. Plug it in, connect it to the Wi-Fi, and Apply again."),
                        None,
                    );
                }
                (serial, allowed)
            },
            |app, (serial, allowed)| {
                if let Some(addr) = allowed {
                    let mut phones = Phones::load();
                    if let Some(p) = phones.by_serial.get_mut(&serial) {
                        info!("desktop: {serial} answers over Wi-Fi at {addr}");
                        p.addr = Some(addr);
                        phones.save();
                    }
                }
                // Applied: what was just set is done NOW, not at the next
                // hour (Max, 2026-10-10: ticked WhatsApp media, applied —
                // "the phone should sync that right away").
                app.desktop.sync_now.insert(serial);
                app.phone_sync_tick();
            },
        );
        Some((config.serial, phone))
    }

    /// Clean the phone, with the settings as they stand in the box (which
    /// are applied): its files of the ticked kinds go to its folder and, each
    /// once its copy is checked, off the phone. On the task pill, with
    /// Cancel; a notification says how it ended.
    fn desktop_config_clean(&mut self) {
        let Some((serial, phone)) = self.desktop_config_apply() else {
            return;
        };
        let name = phone.name.clone();
        let task = self.task_begin(&format!("{} {name}", crate::i18n::tr("Cleaning")));
        info!("desktop: cleaning {name} ({serial})");
        std::thread::spawn(move || {
            use crate::i18n::tr;
            let folder = phone.folder();
            let moved = crate::desktop_phone::reach(&serial, phone.addr.as_deref())
                .and_then(|target| crate::desktop_phone::transfer(&target, &phone.its_kinds(), &folder, true, &task));
            let files = |n: usize| if n == 1 { format!("1 {}", tr("file")) } else { format!("{n} {}", tr("files")) };
            let said = match &moved {
                None => tr("It could not be reached. Plug it in and unlock it, then clean again.").to_owned(),
                Some(m) if task.cancelled() => {
                    format!("{} {} {}", tr("Clean stopped."), files(m.copied.len()), tr("copied; nothing was taken off the phone."))
                }
                Some(m) if m.removed == 0 && m.failed == 0 => tr("There was nothing to clean.").to_owned(),
                Some(m) if m.failed == 0 => format!("{} {}", files(m.removed), tr("moved off the phone. Click to view.")),
                Some(m) => format!(
                    "{} {} {} {}",
                    files(m.removed),
                    tr("moved off the phone;"),
                    files(m.failed),
                    tr("could not be copied and stayed on it. Click to view.")
                ),
            };
            info!("desktop: {name}: {said}");
            let show = folder.is_dir().then(|| format!("{}{}", crate::NOTIFY_OPEN, folder.display()));
            crate::desktop_send_notify_open(&name, "phone", &said, show.as_deref());
        });
    }

    /// Every `SYNC_TICK`: look for the phones that are synced over Wi-Fi,
    /// and sync the ones that are there and due.
    pub(crate) fn phone_sync_tick(&mut self) {
        for (serial, phone) in Phones::load().by_serial {
            if !phone.sync || self.desktop.syncing.contains(&serial) {
                continue;
            }
            let was_there = self.desktop.phones_there.get(&serial).copied().unwrap_or(false);
            // Asked for now (an Apply): due whatever the clock says, and
            // whether or not it is charging — its owner is at it.
            let now = self.desktop.sync_now.remove(&serial);
            let probed = serial.clone();
            self.desktop_off_loop(
                move || {
                    let on_cable = crate::desktop_phone::ready(&probed);
                    let on_wifi = phone.addr.as_deref().is_some_and(crate::desktop_phone::wifi_there);
                    // On its cable and not answering over Wi-Fi: it restarted
                    // (or was never allowed) — allowed again, for next time.
                    let addr = (on_cable && !on_wifi).then(|| crate::desktop_phone::wifi_allow(&probed)).flatten();
                    if !(on_cable || on_wifi) {
                        return (probed, phone, Probe { there: false, target: None, addr });
                    }
                    let since = now_secs().saturating_sub(phone.last_sync.unwrap_or(0));
                    let due = now || since >= phone.every.secs() || (!was_there && since >= SYNC_AGAIN);
                    let target = due
                        .then(|| crate::desktop_phone::reach(&probed, phone.addr.as_deref()))
                        .flatten()
                        .filter(|t| now || !phone.charging_only || crate::desktop_phone::charging(t));
                    (probed, phone, Probe { there: true, target, addr })
                },
                |app, (serial, phone, probe)| {
                    app.desktop.phones_there.insert(serial.clone(), probe.there);
                    if let Some(addr) = probe.addr {
                        let mut phones = Phones::load();
                        if let Some(p) = phones.by_serial.get_mut(&serial) {
                            p.addr = Some(addr);
                            phones.save();
                        }
                    }
                    if let Some(target) = probe.target {
                        app.phone_sync(serial, phone, target);
                    }
                },
            );
        }
    }

    /// Sync one phone now: what it has that is not here comes here (and,
    /// if its owner set that, leaves the phone). On the task pill; it speaks
    /// only when something came.
    fn phone_sync(&mut self, serial: String, phone: crate::phones::Phone, target: String) {
        if !self.desktop.syncing.insert(serial.clone()) {
            return;
        }
        info!("desktop: syncing {} over {target}", phone.name);
        let task = self.task_begin(&format!("{} {}", crate::i18n::tr("Syncing"), phone.name));
        self.desktop_off_loop(
            move || {
                use crate::i18n::tr;
                let folder = phone.folder();
                let moved = crate::desktop_phone::transfer(&target, &phone.its_kinds(), &folder, phone.remove_after_sync, &task);
                if let Some(m) = moved.as_ref().filter(|m| !m.copied.is_empty()) {
                    let n = m.copied.len();
                    let said = if n == 1 {
                        tr("1 new file synced. Click to view.").to_owned()
                    } else {
                        format!("{n} {}", tr("new files synced. Click to view."))
                    };
                    let show = format!("{}{}", crate::NOTIFY_OPEN, folder.display());
                    crate::desktop_send_notify_open(&phone.name, "phone", &said, Some(&show));
                }
                (serial, moved.is_some() && !task.cancelled())
            },
            |app, (serial, synced)| {
                app.desktop.syncing.remove(&serial);
                if synced {
                    let mut phones = Phones::load();
                    if let Some(p) = phones.by_serial.get_mut(&serial) {
                        p.last_sync = Some(now_secs());
                        phones.save();
                    }
                }
            },
        );
    }
}

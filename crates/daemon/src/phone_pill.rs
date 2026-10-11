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
use crate::desktop::Debugging;
use crate::desktop_menu::{Action, Row};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;

/// fa-mobile.
const GLYPH_PHONE: &str = "\u{f10b}";

/// The button in the long pill's air: what a click on it does.
const SYNC: &str = "Sync";
const SET_UP: &str = "Set up";
const STOP: &str = "Stop";
/// (A click on it shows how: Max, 2026-10-11.)
const HOW: &str = "Turn ON";
/// On a phone that shows no files: a click shows how it is made to.
const FILES: &str = "How";
/// How much longer than its word the button is (the task pill's chips
/// have 12; Max, 2026-10-11: "make the button slightly longer").
const CHIP_PAD: f32 = 24.0;
/// The least air either side of that button.
const CHIP_AIR: f32 = 10.0;

/// The air over a group of the box's rows (where the menu has a line).
const GROUP_AIR: f32 = 6.0;
/// The air over a step of a way to do something.
const STEP_AIR: f32 = 10.0;
/// The air under the box's last row.
const BOX_FOOT: f32 = 6.0;

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

/// What the box shows: the phone's menu, or one of the things that menu
/// used to open elsewhere (Max, 2026-10-11: "make the pieces be a part of
/// the box too (properties, configure, and use as camera)").
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Page {
    #[default]
    Menu,
    /// The camera in use: its flash, its lens, its picture.
    Camera,
    /// What this computer does with the phone (`phones.json`), on a copy
    /// until Apply.
    Configure,
    /// What the phone says of itself.
    Properties,
    /// How USB debugging is turned on (or this computer allowed): what a
    /// phone without it is missing here.
    Debugging,
    /// How the phone is made to show its files (it is plugged in and its
    /// folder is empty).
    Files,
}

/// What a click on a line of the box does.
#[derive(Debug, Clone, PartialEq)]
enum Act {
    /// What that row of the phone's menu does.
    Menu(Action),
    Turn(Page),
    Flash,
    Lens(usize),
    Picture(crate::desktop::Picture),
    Sync,
    Kind(usize),
    Every,
    Charging,
    After,
    Apply,
    /// The whole Configure box (the name, Clean).
    Whole,
    Nothing,
}

/// A line of the box.
#[derive(Debug, Clone, PartialEq)]
struct Line {
    label: String,
    /// At the line's right end ("On", "✓").
    value: String,
    act: Act,
    /// Air over it: it begins a group.
    air: bool,
    /// A step of a way to do something: its number, in a round chip before
    /// its words (0: not a step).
    step: u8,
    /// Said under a step, lighter and set in under the step's words.
    sub: bool,
    /// Said lighter (a heading's second line, a closing remark).
    dim: bool,
}

fn line(label: &str, value: &str, act: Act) -> Line {
    Line { label: label.to_owned(), value: value.to_owned(), act, air: false, step: 0, sub: false, dim: false }
}

/// A way to do something, as a page of the box: what and what it costs,
/// then one thing to do a step — a round number before each, air between
/// the steps, what is seen on the phone under the step that shows it — and
/// what it is for, last (Max, 2026-10-11: the first cut, six lines one
/// under the other, was "all close together, hard to understand and
/// follow").
fn guide(title: &str, under: &str, steps: &[(&str, &str)], last: &[&str]) -> Vec<Line> {
    let say = |label: &str| line(label, "", Act::Nothing);
    let mut lines = vec![line("‹  Back", "", Act::Turn(Page::Menu))];
    lines.push(Line { air: true, ..say(title) });
    lines.push(Line { dim: true, ..say(under) });
    for (n, (text, seen)) in steps.iter().enumerate() {
        lines.push(Line { air: true, step: n as u8 + 1, ..say(text) });
        if !seen.is_empty() {
            lines.push(Line { sub: true, ..say(seen) });
        }
    }
    for (n, text) in last.iter().enumerate() {
        lines.push(Line { air: n == 0, dim: true, ..say(text) });
    }
    lines
}

/// How USB debugging is turned on — or, `asked`, this computer allowed on a
/// phone that has it on and is asking.
fn debugging_lines(asked: bool) -> Vec<Line> {
    const FOR: [&str; 2] =
        ["Then it can be mirrored, used as a camera and synced,", "and it shows its battery and shares its internet."];
    if asked {
        guide(
            "Allow this computer",
            "The phone is waiting for your answer.",
            &[
                ("Look at the phone's screen", "It asks: Allow USB debugging?"),
                ("Tick \"Always allow from this computer\"", ""),
                ("Tap Allow", ""),
            ],
            &["No question on its screen? Unplug it and plug it in again."],
        )
    } else {
        guide(
            "Turn on USB debugging",
            "Once, on the phone. It takes a minute.",
            &[
                ("Open Settings, then About phone", ""),
                ("Tap \"Build number\" 7 times", "It says: You are now a developer"),
                ("Go back to Settings, then System", "Open Developer options"),
                ("Switch on \"USB debugging\"", ""),
                ("Unplug the phone and plug it in again", ""),
                ("On the phone, tap Allow", "Tick \"Always allow from this computer\""),
            ],
            &FOR,
        )
    }
}

/// How a phone that is plugged in and shows nothing is made to show its
/// files (Max, 2026-10-11, an old Huawei: "i open one, it is empty… i dont
/// know how to paste a folder or a file on this device").
fn files_lines() -> Vec<Line> {
    guide(
        "See the phone's files",
        "It is plugged in, but it is not showing them yet.",
        &[
            ("Unlock the phone's screen", ""),
            ("Pull down its notifications", "Tap the one about USB: \"Charging this device\""),
            ("Choose \"Transfer files\"", "Some phones call it \"File transfer\" or \"MTP\""),
            ("If it asks to allow access to its data, tap Allow", ""),
        ],
        &["Then open the phone from its icon: its storage is a folder,", "and files are copied and pasted there like anywhere."],
    )
}

/// What the box of a phone that is not there shows
/// (`debug-desktop "phonepill fake"`): each page as it looks, acting on
/// nothing.
fn fake_lines(page: Page) -> Vec<Line> {
    let back = line("‹  Back", "", Act::Turn(Page::Menu));
    let rows: &[(&str, &str, bool)] = match page {
        Page::Menu => &[
            ("Open", "", false),
            ("Open in terminal", "", false),
            ("Mirror screen", "", false),
            ("Stop camera", "", true),
            ("Configure camera", "", false),
            ("Import photos", "", true),
            ("Use phone's internet", "", false),
            ("Eject", "", true),
            ("Configure", "", true),
            ("Properties", "", false),
        ],
        Page::Camera => &[
            ("Flash", "Off", true),
            ("Ultra wide", "", true),
            ("Main", "✓", false),
            ("Telephoto", "", false),
            ("Front", "", false),
            ("1080p · 60", "✓", true),
            ("1080p · 30", "", false),
            ("720p · 60", "", false),
            ("720p · 30", "", false),
        ],
        Page::Configure => &[
            ("Sync over Wi-Fi", "On", true),
            ("Photos and videos", "✓", true),
            ("Music", "✓", false),
            ("Downloads", "✓", false),
            ("Documents", "✓", false),
            ("Recordings", "", false),
            ("WhatsApp media", "✓", false),
            ("How often", "Hourly", true),
            ("Only while charging", "Yes", false),
            ("Take off the phone once synced", "No", false),
            ("Name and clean-up…", "", true),
            ("Apply", "", false),
        ],
        Page::Debugging => return debugging_lines(false),
        Page::Files => return files_lines(),
        Page::Properties => &[
            ("Model", "Google Pixel 8 Pro", true),
            ("Android", "17", false),
            ("Battery", "80%, charging", false),
            ("Storage", "91 GB of 110 GB used", false),
        ],
    };
    let mut lines: Vec<Line> = if page == Page::Menu { Vec::new() } else { vec![back] };
    lines.extend(rows.iter().map(|(label, value, air)| {
        let act = match *label {
            "Configure camera" => Act::Turn(Page::Camera),
            "Configure" => Act::Turn(Page::Configure),
            "Properties" => Act::Turn(Page::Properties),
            _ => Act::Nothing,
        };
        Line { air: *air, ..line(label, value, act) }
    }));
    lines
}

fn yes(on: bool) -> &'static str {
    if on {
        "✓"
    } else {
        ""
    }
}

/// What the pill shows.
#[derive(Debug, Clone, PartialEq)]
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
    /// Whether it lets this computer in over adb (most of what a phone is
    /// good for here goes that way).
    debugging: Debugging,
    /// It is mounted and its folder is empty: it is not showing its files
    /// (locked, or still "charging only").
    no_files: bool,
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
    /// Whether each phone lets this computer in (by serial), as last asked.
    debugging: std::collections::HashMap<String, Debugging>,
    /// The phones (by their folder) that were showing no files when last
    /// looked at.
    no_files: std::collections::HashSet<String>,
    /// Its box (all a phone's menu has): asked open, and how far open.
    box_open: bool,
    box_e: f32,
    /// What the box shows, and its height easing after its page's.
    page: Page,
    box_h: f32,
    /// The Configure page's copy of the phone's settings (its own until
    /// Apply).
    config: Option<crate::phones::Phone>,
    /// What the phone said of itself, for the Properties page (`None`: it
    /// is being asked).
    facts: Option<Vec<(&'static str, String)>>,
    /// The measured widths of the words set at the lines' right ends.
    value_w: std::collections::HashMap<String, f32>,
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
        let path = v.path.to_string_lossy().into_owned();
        let camera = self.desktop.camera_on.as_ref().is_some_and(|c| c.path == path);
        // A phone that gives no serial has no adb to be asked by.
        let debugging = match &serial {
            Some(s) => self.phone_pill.debugging.get(s).copied().unwrap_or(Debugging::Unknown),
            None => Debugging::Off,
        };
        let no_files = self.phone_pill.no_files.contains(&path);
        let state = match &known {
            _ if camera => "Using as a camera".to_owned(),
            _ if v.closed => "Locked".to_owned(),
            _ if no_files => "Not showing its files".to_owned(),
            _ if debugging == Debugging::Off => "USB debugging off".to_owned(),
            _ if debugging == Debugging::NotAllowed => "Not allowed on the phone".to_owned(),
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
            path,
            says: format!("{name} · {state}"),
            battery,
            button: match &known {
                _ if camera => Some(STOP),
                _ if v.closed || syncing => None,
                _ if no_files => Some(FILES),
                _ if matches!(debugging, Debugging::Off | Debugging::NotAllowed) => Some(HOW),
                None => Some(SET_UP),
                Some(_) => Some(SYNC),
            },
            known: known.is_some(),
            serial,
            debugging,
            no_files,
        })
    }

    /// Ask whether the plugged-in phone lets this computer in, and how full
    /// its battery is (off the loop); the pill is written again when it
    /// answers.
    fn phone_pill_ask_battery(&mut self) {
        let Some(now) = self.phone_pill_now() else {
            return;
        };
        let (Some(serial), path) = (now.serial, now.path) else {
            return;
        };
        if self.phone_pill.fake.is_some() || self.phone_pill.asking.as_ref() == Some(&serial) {
            return;
        }
        self.phone_pill.asking = Some(serial.clone());
        self.desktop_off_loop(
            move || {
                let debugging = crate::desktop::phone_debugging(&serial);
                let level = (debugging == Debugging::Ready).then(|| crate::desktop_phone::battery(&serial)).flatten();
                // (Its folder is there and has nothing in it: a phone shows
                // its storage as a folder once it lets its files be seen.)
                let empty = std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none());
                (serial, debugging, level, path, empty)
            },
            |app, (serial, debugging, level, path, empty)| {
                app.phone_pill.asking = None;
                let mut changed = app.phone_pill.debugging.insert(serial.clone(), debugging) != Some(debugging);
                changed |= if empty { app.phone_pill.no_files.insert(path) } else { app.phone_pill.no_files.remove(&path) };
                match level {
                    Some(level) => changed |= app.phone_pill.battery.insert(serial, level) != Some(level),
                    None => changed |= app.phone_pill.battery.remove(&serial).is_some(),
                }
                if changed {
                    app.phone_pill_sync();
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
                    self.phone_pill.chip_w = now.button.map_or(0.0, |word| r.measure_text(word, font_px, TEXT_FONT) + CHIP_PAD);
                    self.phone_pill.measured = key;
                }
            }
            // A phone not asked yet (it has just been plugged in).
            if now.serial.as_ref().is_some_and(|s| !self.phone_pill.debugging.contains_key(s)) {
                self.phone_pill_ask_battery();
            }
            self.phone_pill.shown = Some(now);
        }
        self.schedule_phone_pill_frame();
        self.draw_options();
    }

    /// What is plugged in changed: the pill follows, and the phone is asked
    /// again whether it lets this computer in (turning USB debugging on
    /// makes it drop off its cable and come back).
    pub(crate) fn phone_pill_plugged(&mut self) {
        self.phone_pill_sync();
        self.phone_pill_ask_battery();
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
        let want = f32::from(u8::from((p.want_open || p.box_open) && there > 0.0));
        let (open, b) = ease_toward(p.open, want, dt, animation::MORPH_RATE, settle);
        self.phone_pill.t = t;
        self.phone_pill.open = open;
        if there == 0.0 {
            self.phone_pill.box_open = false;
        }
        let boxed = f32::from(u8::from(self.phone_pill.box_open));
        let (box_e, c) = ease_toward(self.phone_pill.box_e, boxed, dt, animation::MORPH_RATE, animation::SETTLE_ALPHA);
        self.phone_pill.box_e = box_e;
        // (Folded: the next opening starts at the menu.)
        if box_e <= 0.01 && !self.phone_pill.box_open {
            self.phone_pill.page = Page::Menu;
            self.phone_pill.config = None;
        }
        self.phone_box_measure();
        let want_h = self.phone_box_page_h();
        let (box_h, c2) = if self.phone_pill.box_h <= 0.0 {
            (want_h, false)
        } else {
            let (h, moving) = ease_toward(self.phone_pill.box_h, want_h, dt, animation::MORPH_RATE, 0.5);
            (h, moving)
        };
        self.phone_pill.box_h = box_h;
        let c = c || c2;
        self.sync_options_input();
        self.draw_options();
        if a || b || c {
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
        let band = (base * LONGER).max(base + if chip > 0.0 { chip + 2.0 * CHIP_AIR } else { 0.0 });
        // Its box is as wide as it is: while that is open the pill is at
        // least as long as the box's longest line needs.
        if self.phone_pill.box_e <= 0.0 && !self.phone_pill.box_open {
            return band;
        }
        let w = |text: &str| self.phone_pill.value_w.get(text).copied().unwrap_or(0.0);
        let widest = self
            .phone_box_lines()
            .iter()
            .map(|l| {
                let indent = if l.step > 0 || l.sub { self.phone_step_indent() } else { 0.0 };
                indent + w(&l.label) + if l.value.is_empty() { 0.0 } else { w(&l.value) + 2.0 * PILL_PAD_X }
            })
            .fold(0.0, f32::max);
        lerp(band, band.max(widest + 2.0 * PILL_PAD_X), self.phone_pill.box_e)
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
        let band = self.options_pill_h().min(long.h);
        (right - left >= p.chip_w).then(|| Rect::new((left + right - p.chip_w) / 2.0, long.y + (band - h) / 2.0, p.chip_w, h))
    }

    /// The long pill within the whole, fully out.
    fn phone_pill_long(&self, rect: Rect) -> Rect {
        let round = self.phone_pill_round(rect);
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
        Some(Rect::new(right - ph - long, y, ph + long, ph + self.phone_box_h() * p.box_e))
    }

    /// The round pill within the whole (the whole is as tall as its box
    /// makes it; the round pill is always one pill's height).
    fn phone_pill_round(&self, rect: Rect) -> Rect {
        let ph = self.options_pill_h().min(rect.h);
        Rect::new(rect.x + rect.w - ph, rect.y, ph, ph)
    }

    /// The plugged-in phone's volume, its serial and the desktop item it is.
    fn phone_pill_volume(&self) -> Option<(&crate::mounts::Mounted, Option<String>, Option<usize>)> {
        let shown = self.phone_pill.shown.as_ref()?;
        let v = self.desktop.volumes.iter().find(|v| v.path.to_string_lossy() == shown.path.as_str())?;
        let item = self.desktop.items.iter().position(|it| it.path == shown.path);
        Some((v, crate::desktop::phone_serial_of(v), item))
    }

    /// The lines of the box's page.
    fn phone_box_lines(&self) -> Vec<Line> {
        // (A phone that is not there has a box to look at all the same.)
        if self.phone_pill.fake.is_some() {
            return fake_lines(self.phone_pill.page);
        }
        let Some((v, _, _)) = self.phone_pill_volume() else {
            return Vec::new();
        };
        let path = v.path.to_string_lossy().into_owned();
        let back = line("‹  Back", "", Act::Turn(Page::Menu));
        let group = |mut l: Line| {
            l.air = true;
            l
        };
        match self.phone_pill.page {
            // The phone's own menu (`desktop_menu::volume_rows` — one list
            // for both), with what it opened elsewhere turned into pages
            // of this box, and the camera's settings under its Stop.
            Page::Menu => {
                let camera = self.desktop.camera_on.as_ref().is_some_and(|c| c.path == path)
                    || self.desktop.cameras.lock().is_ok_and(|c| c.contains_key(&path));
                let volume = crate::desktop_menu::Volume { phone: true, closed: v.closed, camera, tether: v.tether };
                // Without adb: what the cable alone can do (its folder, its
                // photos by that folder), and the way to the rest (Max,
                // 2026-10-11: "find out if it's not able, offer what is
                // possible, and show the option… for more features").
                let limited = self.phone_pill.shown.as_ref().map(|s| s.debugging);
                let limited = limited.filter(|d| matches!(d, Debugging::Off | Debugging::NotAllowed));
                let mut lines = Vec::new();
                let mut air = false;
                for row in crate::desktop_menu::volume_rows(&volume) {
                    if limited.is_some()
                        && matches!(
                            row,
                            Row::Item {
                                action: Action::Mirror
                                    | Action::Camera
                                    | Action::CameraStop
                                    | Action::Tether
                                    | Action::Configure
                                    | Action::Properties,
                                ..
                            }
                        )
                    {
                        continue;
                    }
                    let (label, action) = match row {
                        Row::Sep => {
                            air = true;
                            continue;
                        }
                        Row::Note(_) => continue,
                        Row::Item { label, action, .. } => (crate::i18n::tr(label).to_owned(), action),
                        Row::Target { label, action } => (label, action),
                    };
                    let act = match action {
                        Action::Configure => Act::Turn(Page::Configure),
                        Action::Properties => Act::Turn(Page::Properties),
                        other => Act::Menu(other),
                    };
                    lines.push(Line { air, ..line(&label, "", act) });
                    air = false;
                    if action == Action::CameraStop {
                        lines.push(line("Configure camera", "", Act::Turn(Page::Camera)));
                    }
                }
                if self.phone_pill.shown.as_ref().is_some_and(|s| s.no_files) {
                    lines.push(group(line("It shows no files: what to do on the phone", "", Act::Turn(Page::Files))));
                }
                match limited {
                    Some(Debugging::NotAllowed) => {
                        lines.push(group(line("Allow this computer on the phone for more features", "", Act::Turn(Page::Debugging))));
                    }
                    Some(_) => {
                        lines.push(group(line("Turn on USB debugging for more features", "", Act::Turn(Page::Debugging))));
                    }
                    None => {}
                }
                lines
            }
            Page::Debugging => {
                debugging_lines(self.phone_pill.shown.as_ref().is_some_and(|s| s.debugging == Debugging::NotAllowed))
            }
            Page::Files => files_lines(),
            Page::Camera => {
                let mut lines = vec![back];
                let Some((on, lenses, pictures)) = self.camera_choices() else {
                    lines.push(group(line("The camera is off", "", Act::Nothing)));
                    return lines;
                };
                if self.camera_has_torch() {
                    lines.push(group(line("Flash", if on.torch { "On" } else { "Off" }, Act::Flash)));
                }
                for (n, name) in lenses.iter().enumerate() {
                    let l = line(name, yes(n == on.lens), Act::Lens(n));
                    lines.push(if n == 0 { group(l) } else { l });
                }
                for (k, picture) in pictures.iter().enumerate() {
                    let l = line(&picture.label(), yes(*picture == on.picture), Act::Picture(*picture));
                    lines.push(if k == 0 { group(l) } else { l });
                }
                lines
            }
            Page::Configure => {
                let mut lines = vec![back];
                let Some(phone) = self.phone_pill.config.as_ref() else {
                    return lines;
                };
                lines.push(group(line("Sync over Wi-Fi", if phone.sync { "On" } else { "Off" }, Act::Sync)));
                for (i, kind) in crate::phones::KINDS.iter().enumerate() {
                    let l = line(kind.label, yes(phone.kinds.iter().any(|k| k == kind.key)), Act::Kind(i));
                    lines.push(if i == 0 { group(l) } else { l });
                }
                let every = match phone.every {
                    crate::phones::Every::Hour => "Hourly",
                    crate::phones::Every::Quarter => "Every 15 min",
                    crate::phones::Every::Day => "Daily",
                };
                lines.push(group(line("How often", every, Act::Every)));
                lines.push(line("Only while charging", if phone.charging_only { "Yes" } else { "No" }, Act::Charging));
                lines.push(line("Take off the phone once synced", if phone.remove_after_sync { "Yes" } else { "No" }, Act::After));
                lines.push(group(line("Name and clean-up…", "", Act::Whole)));
                lines.push(line("Apply", "", Act::Apply));
                lines
            }
            Page::Properties => {
                let mut lines = vec![back];
                match &self.phone_pill.facts {
                    None => lines.push(group(line("Asking the phone…", "", Act::Nothing))),
                    Some(facts) if facts.is_empty() => lines.push(group(line("It would not say", "", Act::Nothing))),
                    Some(facts) => {
                        for (n, (key, value)) in facts.iter().enumerate() {
                            let l = line(key, value, Act::Nothing);
                            lines.push(if n == 0 { group(l) } else { l });
                        }
                    }
                }
                lines
            }
        }
    }

    /// How far a step's words stand in from the line's own start: its
    /// number's chip and a gap.
    fn phone_step_indent(&self) -> f32 {
        LINE_PX * self.options_scale() + 8.0
    }

    /// Measure the words at the lines' right ends that are not measured yet
    /// (they are set from the right, by their width).
    fn phone_box_measure(&mut self) {
        if self.phone_pill.box_e <= 0.0 && !self.phone_pill.box_open {
            return;
        }
        let font_px = FONT_PX * self.options_scale();
        let new: Vec<String> = self
            .phone_box_lines()
            .into_iter()
            .flat_map(|l| [l.value, l.label])
            .filter(|v| !v.is_empty() && !self.phone_pill.value_w.contains_key(v))
            .collect();
        if let Some(r) = self.options_renderer.as_mut() {
            for value in new {
                let w = r.measure_text(&value, font_px, TEXT_FONT);
                self.phone_pill.value_w.insert(value, w);
            }
        }
    }

    /// The box's lines, each with its place under the long pill (`long`:
    /// the long pill and its box, whole).
    fn phone_box_placed(&self, long: Rect) -> Vec<(Rect, Line)> {
        let ph = self.options_pill_h();
        let mut y = long.y + ph;
        self.phone_box_lines()
            .into_iter()
            .map(|l| {
                if l.air {
                    y += if l.step > 0 { STEP_AIR } else { GROUP_AIR };
                }
                let rect = Rect::new(long.x, y, long.w, ph);
                y += ph;
                (rect, l)
            })
            .collect()
    }

    /// The height of the box's page, fully open.
    fn phone_box_page_h(&self) -> f32 {
        let ph = self.options_pill_h();
        let lines = self.phone_box_lines();
        if lines.is_empty() {
            return 0.0;
        }
        lines
            .iter()
            .map(|l| ph + if !l.air { 0.0 } else if l.step > 0 { STEP_AIR } else { GROUP_AIR })
            .sum::<f32>()
            + BOX_FOOT
    }

    /// The box's height now (it eases from one page's to the next's).
    fn phone_box_h(&self) -> f32 {
        if self.phone_pill.box_h > 0.0 {
            self.phone_pill.box_h
        } else {
            self.phone_box_page_h()
        }
    }

    /// Turn the box's page; what the page needs is fetched.
    fn phone_box_turn(&mut self, page: Page) {
        self.phone_pill.page = page;
        match page {
            Page::Configure => {
                if let Some((v, serial, _)) = self.phone_pill_volume() {
                    let phone = crate::phones::Phones::load().of(serial.as_deref().unwrap_or_default(), &v.name);
                    self.phone_pill.config = Some(phone);
                }
            }
            Page::Properties => {
                self.phone_pill.facts = None;
                if let Some(serial) = self.phone_pill_volume().and_then(|(_, serial, _)| serial) {
                    self.desktop_off_loop(
                        move || crate::desktop_phone::facts(&serial),
                        |app, facts| {
                            app.phone_pill.facts = Some(facts);
                            app.schedule_phone_pill_frame();
                            app.draw_options();
                        },
                    );
                } else {
                    self.phone_pill.facts = Some(Vec::new());
                }
            }
            Page::Menu | Page::Camera | Page::Debugging | Page::Files => {}
        }
        self.schedule_phone_pill_frame();
        self.sync_options_input();
        self.draw_options();
    }

    /// A click on a line of the box.
    fn phone_box_act(&mut self, act: Act, at: (f32, f32)) {
        let item = self.phone_pill_volume().and_then(|(_, _, item)| item);
        let fold = |app: &mut App| {
            app.phone_pill.box_open = false;
            app.schedule_phone_pill_frame();
        };
        match act {
            Act::Nothing => {}
            Act::Turn(page) => self.phone_box_turn(page),
            // Use as camera: it starts, and the box turns to its settings.
            Act::Menu(Action::Camera) => {
                self.desktop_menu_act(Action::Camera, item, at);
                self.phone_box_turn(Page::Camera);
            }
            Act::Menu(Action::CameraStop) => self.desktop_menu_act(Action::CameraStop, item, at),
            Act::Menu(action) => {
                fold(self);
                self.desktop_menu_act(action, item, at);
            }
            Act::Flash => {
                let on = self.desktop.camera_on.as_ref().is_some_and(|c| c.torch);
                self.camera_set(None, None, Some(!on));
            }
            Act::Lens(n) => self.camera_set(Some(n), None, None),
            Act::Picture(picture) => self.camera_set(None, Some(picture), None),
            Act::Sync | Act::Kind(_) | Act::Every | Act::Charging | Act::After => {
                if let Some(phone) = self.phone_pill.config.as_mut() {
                    match act {
                        Act::Sync => phone.sync = !phone.sync,
                        Act::Kind(i) => {
                            if let Some(kind) = crate::phones::KINDS.get(i) {
                                match phone.kinds.iter().position(|k| k == kind.key) {
                                    Some(at) => {
                                        phone.kinds.remove(at);
                                    }
                                    None => phone.kinds.push(kind.key.to_owned()),
                                }
                            }
                        }
                        Act::Every => phone.every = phone.every.next(),
                        Act::Charging => phone.charging_only = !phone.charging_only,
                        _ => phone.remove_after_sync = !phone.remove_after_sync,
                    }
                }
            }
            // Apply: by the Configure box's own way of applying (it allows
            // the phone over Wi-Fi, saves, and syncs what was just set).
            Act::Apply => {
                let (Some(phone), Some((v, serial, _))) = (self.phone_pill.config.take(), self.phone_pill_volume()) else {
                    return;
                };
                let config = crate::desktop_config::Config::of(
                    v.path.to_string_lossy().into_owned(),
                    serial.unwrap_or_default(),
                    v.name.clone(),
                    phone,
                );
                self.desktop.config = Some(config);
                self.desktop_config_apply();
                self.phone_box_turn(Page::Menu);
            }
            Act::Whole => {
                fold(self);
                if let Some(i) = item {
                    self.desktop_open_config(i, at);
                }
            }
        }
        self.phone_pill_sync();
    }

    /// A scroll over the pill: down opens its box, up folds it (the task
    /// pill's gesture and its sign).
    pub(crate) fn phone_pill_axis(&mut self, value: f32) {
        if value.abs() < crate::options::SCROLL_DEADZONE {
            return;
        }
        let open = value < 0.0 && !self.phone_box_lines().is_empty();
        if open != self.phone_pill.box_open {
            self.phone_pill.box_open = open;
            self.schedule_phone_pill_frame();
            self.sync_options_input();
            self.draw_options();
        }
    }

    /// The box is open (the bar's pointer region reaches down over it).
    pub(crate) fn phone_box_open(&self) -> bool {
        self.phone_pill.box_open
    }

    /// How far down the pointer region must reach while the box is open.
    pub(crate) fn phone_box_input_bottom(&self) -> f32 {
        crate::options::PILL_MARGIN_Y + self.options_pill_h() + self.phone_box_h()
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
        if self.phone_pill.box_open {
            self.phone_pill.box_open = false;
            self.schedule_phone_pill_frame();
        }
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
        let round = self.phone_pill_round(rect);
        // A row of the box: what that row of the phone's menu does, and the
        // box folds (a box it opens — Configure, Properties — stands under
        // the pill).
        if ptr.1 > round.y + round.h {
            let long = self.phone_pill_long(rect);
            let hit = self.phone_box_placed(long).into_iter().find(|(r, _)| r.contains(ptr));
            if let Some((_, l)) = hit {
                tracing::info!("desktop: \"{}\" from the bar's phone pill", l.label);
                self.phone_box_act(l.act, (long.x, round.y + round.h + 8.0));
            }
            return;
        }
        if ptr.0 < round.x {
            // The long pill: its button says what a click does.
            match self.phone_pill.shown.as_ref().and_then(|s| s.button) {
                Some(STOP) => {
                    if let Some(path) = self.phone_pill.shown.as_ref().map(|s| s.path.clone()) {
                        self.camera_stop(&path);
                    }
                }
                Some(SET_UP) => {
                    self.phone_pill.box_open = true;
                    self.phone_box_turn(Page::Configure);
                }
                Some(HOW) => {
                    self.phone_pill.box_open = true;
                    self.phone_box_turn(Page::Debugging);
                }
                Some(FILES) => {
                    self.phone_pill.box_open = true;
                    self.phone_box_turn(Page::Files);
                }
                Some(_) => self.phone_pill_sync_now(),
                None => {}
            }
            return;
        }
        if let Some(path) = self.phone_pill.shown.as_ref().map(|s| s.path.clone()) {
            tracing::info!("desktop: eject from the bar's phone pill");
            self.desktop_eject(&path);
        }
    }

    /// Sync the plugged-in phone now, by its cable, whatever its settings
    /// say of when.
    fn phone_pill_sync_now(&mut self) {
        let Some(shown) = self.phone_pill_now() else {
            return;
        };
        if !shown.known {
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
                    debugging: Debugging::Ready,
                    no_files: false,
                    serial: None,
                    known: true,
                });
            }
            "gone" => self.phone_pill.fake = None,
            "open" => self.phone_pill.forced = Some(true),
            // The box, as a scroll down and a scroll up do.
            "box" => self.phone_pill.box_open = !self.phone_box_lines().is_empty(),
            // A page of the box, and a click on its `n`th line (never one
            // of the menu's own rows: those act on the phone).
            "camera" => self.phone_box_turn(Page::Camera),
            "configure" => self.phone_box_turn(Page::Configure),
            "properties" => self.phone_box_turn(Page::Properties),
            "debugging" => self.phone_box_turn(Page::Debugging),
            "files" => self.phone_box_turn(Page::Files),
            "menu" => self.phone_box_turn(Page::Menu),
            "line" => {
                let act = rest.trim().parse::<usize>().ok().and_then(|n| self.phone_box_lines().into_iter().nth(n)).map(|l| l.act);
                match act {
                    Some(Act::Menu(_)) | None => return "phonepill line <n of a page's line that is not a menu row>".to_owned(),
                    Some(act) => self.phone_box_act(act, (0.0, 0.0)),
                }
            }
            "fold" => self.phone_pill.box_open = false,
            "close" => self.phone_pill.forced = Some(false),
            "free" => self.phone_pill.forced = None,
            _ => return "phonepill fake [words]|gone|open|close|free|box|fold|menu|camera|configure|properties|line <n>".to_owned(),
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
        let round = self.phone_pill_round(rect);
        let ptr = self.options_ptr.filter(|_| self.options_hover == Some(PillId::Phone));
        let on_round = ptr.is_some_and(|p| p.0 >= round.x && p.1 <= round.y + round.h);

        // The long pill: from behind the round one, leftward.
        let out = a * ((p.open - 0.1) / 0.6).clamp(0.0, 1.0);
        let long = self.phone_pill_long(rect);
        if out > 0.004 && long.w > 2.0 {
            // The long pill, and under it as much of its box as is open:
            // the task pill's morph (and the player's) — the corner runs
            // from the stadium to the box's, the fill from the pill's wash
            // to the panel's colour.
            let ph = self.options_pill_h().min(long.h);
            let e = p.box_e;
            let solid = 1.0 - (1.0 - e).powi(3);
            let radius = lerp(ph / 2.0, crate::clipboard::BOX_RADIUS, e);
            push_neumorph(scene, long, radius, bright, out);
            let wash = self.options_rest_wash();
            let (panel, box_ink) = self.clip_box_surface();
            scene.rects.push(RectInst {
                rect: long,
                radius,
                color: lerp4(
                    [wash[0], wash[1], wash[2], wash[3] * out],
                    [panel[0], panel[1], panel[2], self.box_panel_alpha() * out],
                    solid,
                ),
                glass: 0.0,
                border: 0.0,
            });
            let ink = lerp4(ink, box_ink, solid);
            // The box's rows: the phone's menu, a line each, the one under
            // the pointer on the hover wash. A row is drawn once the opening
            // box has room for all of it.
            if e > 0.01 {
                let hover = self.options_hover_wash();
                for (row, l) in self.phone_box_placed(long) {
                    if row.y + row.h > long.y + long.h + 0.5 {
                        break;
                    }
                    if l.act != Act::Nothing && ptr.is_some_and(|p| row.contains(p)) {
                        scene.rects.push(RectInst {
                            rect: Rect::new(row.x + 3.0, row.y, row.w - 6.0, row.h),
                            radius: row.h / 2.0,
                            color: [hover[0], hover[1], hover[2], hover[3] * solid * out],
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    let ty = row.y + (row.h - LINE_PX * s) / 2.0;
                    // A step: its number in a round chip (the bell's amber),
                    // its words set in after it; what is said under a
                    // step, lighter, in line with the step's words.
                    let indent = if l.step > 0 || l.sub { self.phone_step_indent() } else { 0.0 };
                    if l.step > 0 {
                        let d = LINE_PX * s;
                        let chip = Rect::new(row.x + PILL_PAD_X, row.y + (row.h - d) / 2.0, d, d);
                        let amber = crate::notif::AMBER;
                        scene.rects.push(RectInst {
                            rect: chip,
                            radius: d / 2.0,
                            color: [amber[0], amber[1], amber[2], 0.3 * solid * out],
                            glass: 0.0,
                            border: 0.0,
                        });
                        scene.labels.push(Label {
                            text: l.step.to_string(),
                            pos: (chip.x + chip.w / 2.0, ty),
                            max_w: 4096.0,
                            font_px: FONT_PX * s,
                            line_px: LINE_PX * s,
                            centered: true,
                            dim: false,
                            cache: true,
                            family: TEXT_FONT,
                            color: Some([ink[0], ink[1], ink[2], ink[3] * solid * out]),
                            clip: Some(row),
                        });
                    }
                    let mut word = |text: String, x: f32, alpha: f32| {
                        scene.labels.push(Label {
                            text,
                            pos: (x, ty),
                            max_w: 4096.0,
                            font_px: FONT_PX * s,
                            line_px: LINE_PX * s,
                            centered: false,
                            dim: false,
                            cache: true,
                            family: TEXT_FONT,
                            color: Some([ink[0], ink[1], ink[2], ink[3] * solid * out * alpha]),
                            clip: Some(row),
                        });
                    };
                    word(l.label, row.x + PILL_PAD_X + indent, if l.sub || l.dim { 0.6 } else { 1.0 });
                    if !l.value.is_empty() {
                        // (Set from the right, by its measured width.)
                        let w = p.value_w.get(&l.value).copied().unwrap_or(l.value.chars().count() as f32 * FONT_PX * s * 0.56);
                        word(l.value, row.x + row.w - PILL_PAD_X - w, 0.7);
                    }
                }
            }
            // (From here on, the band: the long pill itself.)
            let long = Rect::new(long.x, long.y, long.w, ph);
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

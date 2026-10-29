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

use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::animation::{self, ease_toward, lerp, lerp4};
use crate::desktop::Debugging;
use crate::desktop_menu::{Action, Menu, MenuKey, MenuPaint, Row};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{hover_grow, push_neumorph, PillId, FONT_PX, LINE_PX, OPTION_GAP, PILL_PAD_X, TEXT_FONT};
use crate::App;
use crate::i18n::tr;

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
/// (6, with rows as low as the pill, until Max, 2026-10-11: "the pill box
/// is too small, everything is too close".)
const GROUP_AIR: f32 = 12.0;
/// A row of the box, in pill-heights.
const ROW_H: f32 = 1.35;
/// The air between the long pill and its box's first row.
const BOX_HEAD: f32 = 8.0;
/// The box at its narrowest.
const BOX_MIN_W: f32 = 340.0;
/// How far the box's words stand in from its edge.
const BOX_PAD: f32 = 18.0;
/// The air over a step of a way to do something.
const STEP_AIR: f32 = 14.0;
/// The air under the box's last row.
const BOX_FOOT: f32 = 12.0;

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
    /// The phone's Configure box itself — the one its menu opens
    /// (`desktop_config.rs`) — shown here in place of the lines (Max,
    /// 2026-10-11: "the configure box has to be the same one as the one on
    /// the menu").
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

/// What a row of the phone's menu does in the box: what it opened
/// elsewhere turned into a page of the box, the rest the phone's.
fn menu_act(action: Action) -> Act {
    match action {
        Action::Configure => Act::Turn(Page::Configure),
        Action::Properties => Act::Turn(Page::Properties),
        Action::CameraSettings => Act::Turn(Page::Camera),
        Action::HelpDebugging => Act::Turn(Page::Debugging),
        Action::HelpFiles => Act::Turn(Page::Files),
        other => Act::Menu(other),
    }
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
    // (Steps come as constant tables: translated here, as they become lines.)
    let say = |label: &str| line(crate::i18n::tr_dyn(label), "", Act::Nothing);
    let mut lines = vec![line(tr("‹  Back"), "", Act::Turn(Page::Menu))];
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
        tr("See the phone's files"),
        tr("It is plugged in, but it is not showing them yet."),
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
        Page::Configure => &[("(The phone's Configure box, with a phone plugged in)", "", true)],
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
    /// Each phone's Android (its API level, by serial): the camera needs 12
    /// (level 31) — an Android 7 Huawei was offered it and scrcpy gave up
    /// (2026-10-11).
    android: std::collections::HashMap<String, u32>,
    /// The phones (by their folder) that were showing no files when last
    /// looked at.
    no_files: std::collections::HashSet<String>,
    /// Its box (all a phone's menu has): asked open, and how far open.
    box_open: bool,
    box_e: f32,
    /// What the box shows, and its height easing after its page's.
    page: Page,
    box_h: f32,
    /// How far its page is scrolled (a page taller than the bar's boxes
    /// may be — the Configure box is).
    scroll: f32,
    /// What the phone said of itself, for the Properties page (`None`: it
    /// is being asked).
    facts: Option<Vec<(&'static str, String)>>,
    /// The measured widths of the words set at the lines' right ends.
    value_w: std::collections::HashMap<String, f32>,
    /// THE PHONE'S MENU in the box (Max, 2026-10-21: "in the one menu
    /// look"): the one builder's menu, held by the box (`Menu::fit`) and
    /// drawn by its painter — symbols, the quiet tint, the keys, the size.
    menu: Option<Menu>,
    /// Its symbols in the bar's icon array (after the notifications', the
    /// clipboard's and the covers'): their keys and pixels.
    glyph_keys: Vec<String>,
    /// Glyph key → its layer in the bar's icon array.
    pub(crate) glyph_layer: std::collections::HashMap<String, u32>,
    /// The box holds the keyboard (its menu's keys; ← and Escape on a page).
    keyboard_held: bool,
}

/// "5 min ago", "2 h ago", "3 days ago".
fn ago(secs: u64) -> String {
    match secs {
        0..=89 => tr("just now").to_owned(),
        90..=5399 => crate::i18n::fill(tr("{0} min ago"), &[("0", &((secs + 30) / 60))]),
        5400..=129_599 => crate::i18n::fill(tr("{0} h ago"), &[("0", &((secs + 1800) / 3600))]),
        _ => crate::i18n::fill(tr("{0} days ago"), &[("0", &((secs + 43_200) / 86_400))]),
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
            _ if camera => tr("Using as a camera").to_owned(),
            _ if v.closed => tr("Locked").to_owned(),
            _ if no_files => tr("Not showing its files").to_owned(),
            _ if debugging == Debugging::Off => tr("USB debugging off").to_owned(),
            _ if debugging == Debugging::NotAllowed => tr("Not allowed on the phone").to_owned(),
            _ if syncing => tr("Syncing…").to_owned(),
            None => tr("Not set up").to_owned(),
            Some(p) if !p.sync => tr("Sync off").to_owned(),
            Some(p) => match p.last_sync {
                Some(at) => crate::i18n::fill(tr("Synced {0}"), &[("0", &(ago(crate::desktop_config::now_secs().saturating_sub(at))))]),
                None => tr("Not synced yet").to_owned(),
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
                let android = (debugging == Debugging::Ready)
                    .then(|| crate::desktop_phone::android_level(&serial))
                    .flatten();
                (serial, debugging, level, path, empty, android)
            },
            |app, (serial, debugging, level, path, empty, android)| {
                app.phone_pill.asking = None;
                if let Some(android) = android {
                    app.phone_pill.android.insert(serial.clone(), android);
                }
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
                    self.phone_pill.chip_w = now.button.map_or(0.0, |word| r.measure_text(tr(word), font_px, TEXT_FONT) + CHIP_PAD);
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
            self.phone_pill.scroll = 0.0;
            self.phone_config_drop();
        }
        self.phone_box_measure();
        self.phone_config_place();
        let want_h = self.phone_box_page_h().min(self.phone_box_max_h());
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
        // ONE width for every page, the Configure box's (a page wider
        // than that makes it wider): a box that changed size from page to
        // page shrank from under the pointer that had just clicked, and
        // with the pointer off it the visit was over — Max, 2026-10-11:
        // "clicking properties the box gets smaller, when I move my
        // pointer it goes away".
        let wanted = crate::desktop_config::WIDTH.max(widest + 2.0 * BOX_PAD).max(BOX_MIN_W);
        lerp(band, band.max(wanted), self.phone_pill.box_e)
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

    /// What the plugged-in phone `v` cannot do here, as last found out
    /// (`phone_pill_ask_battery`): for its menu on the desktop and for this
    /// pill's box alike.
    pub(crate) fn phone_limits(&self, v: &crate::mounts::Mounted) -> crate::desktop_menu::Limits {
        let serial = crate::desktop::phone_serial_of(v);
        let debugging = match &serial {
            Some(s) => self.phone_pill.debugging.get(s).copied().unwrap_or(Debugging::Unknown),
            None => Debugging::Off,
        };
        crate::desktop_menu::Limits {
            debugging_off: debugging == Debugging::Off,
            not_allowed: debugging == Debugging::NotAllowed,
            no_files: self.phone_pill.no_files.contains(v.path.to_string_lossy().as_ref()),
            old_camera: serial.and_then(|s| self.phone_pill.android.get(&s).copied()).is_some_and(|level| level < 31),
        }
    }

    /// From the phone's menu on the desktop: this pill's box, open at the
    /// page that menu's row is about (the camera's settings, how USB
    /// debugging is turned on, how the phone is made to show its files).
    pub(crate) fn phone_pill_show(&mut self, action: Action) {
        let page = match action {
            Action::CameraSettings => Page::Camera,
            Action::HelpDebugging => Page::Debugging,
            Action::HelpFiles => Page::Files,
            _ => return,
        };
        self.phone_pill.box_open = true;
        self.phone_box_turn(page);
    }

    /// What the phone's menu is made from (`None`: no phone). A phone that
    /// is not there (`phonepill fake`) is one using its camera, all its
    /// rows offered (`fakehelp`: one whose USB debugging is off, its help
    /// rows).
    fn phone_menu_ctx(&self) -> Option<crate::desktop_menu::Ctx> {
        use crate::desktop_menu::{Ctx, Limits, On, Volume};
        if let Some(fake) = self.phone_pill.fake.as_ref() {
            let help = fake.debugging != Debugging::Ready;
            let volume = Volume {
                phone: true,
                camera: !help,
                limits: Limits { debugging_off: help, ..Default::default() },
                ..Default::default()
            };
            return Some(Ctx { on: On::Phone, volume, ..Default::default() });
        }
        let (v, _, _) = self.phone_pill_volume()?;
        let path = v.path.to_string_lossy().into_owned();
        let camera = self.desktop.camera_on.as_ref().is_some_and(|c| c.path == path)
            || self.desktop.cameras.lock().is_ok_and(|c| c.contains_key(&path));
        let limits = self.phone_limits(v);
        let volume = Volume { phone: true, closed: v.closed, camera, tether: v.tether, limits };
        Some(Ctx { on: On::Phone, volume, ..Default::default() })
    }

    /// The box's menu made (again when what it is made of changed), held in
    /// the box as it stands now, at the menus' size; its symbols in the
    /// bar's icon array; the keyboard held while the box is open. Called
    /// before every frame of the bar.
    pub(crate) fn phone_menu_sync(&mut self) {
        let open = self.phone_pill.box_open || self.phone_pill.box_e > 0.01;
        let ctx = (open && self.phone_pill.page == Page::Menu).then(|| self.phone_menu_ctx()).flatten();
        // (Where the pill stands is asked only with a box open on its menu.)
        match ctx.and_then(|ctx| Some((ctx, self.phone_pill_on_bar()?))) {
            Some((ctx, rect)) => {
                let long = self.phone_pill_long(rect);
                let ph = self.options_pill_h().min(long.h);
                let top = long.y + ph + BOX_HEAD;
                let area = Rect::new(long.x, top, long.w, (long.y + long.h - BOX_FOOT - top).max(1.0));
                if self.phone_pill.menu.as_ref().is_none_or(|m| m.ctx != ctx) {
                    self.phone_pill.menu = Some(Menu::new(ctx, None, false, (area.x, area.y), area.x + area.w, area.y + area.h));
                }
                let scale = crate::desktop_menu::scale();
                let e = self.phone_pill.box_e;
                if let Some(menu) = self.phone_pill.menu.as_mut() {
                    menu.scale = scale;
                    menu.t = e.clamp(0.0, 1.0);
                    menu.fit(area);
                }
            }
            _ => self.phone_pill.menu = None,
        }
        self.phone_glyphs_sync();
        let want = self.phone_pill.box_open && self.phone_pill.shown.is_some() && self.phone_pill.page != Page::Configure;
        self.set_phone_keyboard(want);
    }

    /// The symbols the box's menu wears, put in the bar's icon array (from
    /// the pixels the desktop read of them ahead of its first menu); gone
    /// again with the box.
    fn phone_glyphs_sync(&mut self) {
        let keys: Vec<String> = self.phone_pill.menu.as_ref().map_or_else(Vec::new, |m| {
            m.glyphs().into_iter().map(crate::glyphs::key).filter(|k| self.symbols.contains_key(k)).collect()
        });
        if keys == self.phone_pill.glyph_keys && keys.iter().all(|k| self.phone_pill.glyph_layer.contains_key(k)) {
            return;
        }
        if self.options_renderer.is_none() {
            return; // (the next sync, with the bar's renderer, places them)
        }
        for key in &keys {
            if self.phone_pill.glyph_layer.contains_key(key) {
                continue;
            }
            let Some(chain) = self.symbols.get(key).cloned() else {
                continue;
            };
            if let Some(layer) = self.options_icon_put(crate::options_icons::Owner::Glyph(key.clone()), &chain) {
                self.phone_pill.glyph_layer.insert(key.clone(), layer);
            }
        }
        self.phone_pill.glyph_keys = keys;
    }

    /// Take or give back the keyboard (the bar's surface is shared: the
    /// clipboard's box or the gear's may hold it too).
    fn set_phone_keyboard(&mut self, want: bool) {
        if want == self.phone_pill.keyboard_held {
            return;
        }
        let others = self.clip.keyboard_held
            || self.gear.keyboard_held
            || self.desktop.config.as_ref().is_some_and(|c| c.hosted && c.editing);
        if want {
            self.cancel_keyboard_handback(crate::KbSurface::Options);
        } else if !others && !self.keyboard_handback_armed(crate::KbSurface::Options) {
            self.begin_keyboard_handback(crate::KbSurface::Options, None);
        }
        self.phone_pill.keyboard_held = want;
        tracing::debug!("phone pill: keyboard grab {}", if want { "on" } else { "off" });
        let any = self.options_keys_wanted();
        if let Some(layer) = &self.options_layer {
            crate::surface::set_interactive(layer, any);
        }
    }

    /// Whether the box holds the keyboard now.
    pub(crate) fn phone_box_keys(&self) -> bool {
        self.phone_pill.keyboard_held
    }

    /// A key while the box holds the keyboard: its menu's keys (`Menu::key`)
    /// on the first page, ← / Escape / Backspace back to it on the others;
    /// Ctrl +/−/0 the menus' size.
    pub(crate) fn phone_box_key(&mut self, keysym: Keysym, utf8: Option<&str>) {
        if self.modifiers.ctrl {
            match keysym {
                Keysym::plus | Keysym::equal | Keysym::KP_Add => {
                    self.zoom("menu", "in");
                }
                Keysym::minus | Keysym::KP_Subtract => {
                    self.zoom("menu", "out");
                }
                Keysym::_0 | Keysym::KP_0 => {
                    self.zoom("menu", "reset");
                }
                _ => {}
            }
            return;
        }
        if self.phone_pill.page != Page::Menu {
            if matches!(keysym, Keysym::Left | Keysym::Escape | Keysym::BackSpace) {
                self.phone_box_turn(Page::Menu);
            }
            return;
        }
        let ptr = self.options_ptr;
        let Some(menu) = self.phone_pill.menu.as_mut() else {
            return;
        };
        match menu.key(keysym, utf8, ptr) {
            MenuKey::Moved => self.draw_options(),
            MenuKey::Choose(row) => {
                let Some(action) = menu.action(row) else {
                    return;
                };
                let act = menu_act(action);
                // (A phone that is not there turns its pages, and does
                // nothing else.)
                if self.phone_pill.fake.is_some() && matches!(act, Act::Menu(_)) {
                    return;
                }
                let at = menu.rect;
                tracing::info!("desktop: {action:?} from the bar's phone pill, by its keys");
                self.phone_box_act(act, (at.x, at.y));
            }
            MenuKey::Close => {
                self.phone_pill.box_open = false;
                self.schedule_phone_pill_frame();
                self.sync_options_input();
            }
            MenuKey::Back | MenuKey::Nothing => {}
        }
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
            // The phone's menu — the very rows its menu on the desktop has
            // (`desktop_menu::volume_rows`, told what this phone cannot do),
            // with what that menu opens elsewhere turned into pages of this
            // box.
            Page::Menu => {
                let camera = self.desktop.camera_on.as_ref().is_some_and(|c| c.path == path)
                    || self.desktop.cameras.lock().is_ok_and(|c| c.contains_key(&path));
                let limits = self.phone_limits(v);
                let volume =
                    crate::desktop_menu::Volume { phone: true, closed: v.closed, camera, tether: v.tether, limits };
                let ctx = crate::desktop_menu::Ctx { on: crate::desktop_menu::On::Phone, volume, ..Default::default() };
                let mut lines = Vec::new();
                let mut air = false;
                for row in crate::desktop_menu::rows_for(&ctx) {
                    let l = match row {
                        Row::Sep => {
                            air = true;
                            continue;
                        }
                        Row::Search(_) => continue,
                        // (Said, not left out: a row that is simply not
                        // there reads as something broken.)
                        Row::Note(text) => Line { dim: true, ..line(&text, "", Act::Nothing) },
                        Row::Item { label, action, .. } => line(&label, "", menu_act(action)),
                    };
                    lines.push(Line { air, ..l });
                    air = false;
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
                    lines.push(group(line("Flash", if on.torch { "On" } else { tr("Off") }, Act::Flash)));
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
            // (The Configure box draws itself: no lines of this box's.)
            Page::Configure => Vec::new(),
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
        let mut y = long.y + ph + BOX_HEAD - self.phone_pill.scroll;
        let mut first = true;
        self.phone_box_lines()
            .into_iter()
            .map(|l| {
                if l.air && !first {
                    y += if l.step > 0 { STEP_AIR } else { GROUP_AIR };
                }
                first = false;
                // (What is said under a step keeps close to it.)
                let h = if l.sub || l.dim { ph } else { ph * ROW_H };
                let rect = Rect::new(long.x, y, long.w, h);
                y += h;
                (rect, l)
            })
            .collect()
    }

    /// The height of the box's page, fully open.
    fn phone_box_page_h(&self) -> f32 {
        if let Some(config) = self.phone_config() {
            return config.built([1.0; 4], [0.0; 4]).2;
        }
        // (Laid out from nowhere in particular: only how far down the last
        // line ends is wanted — and where the pill is depends on this.)
        let ph = self.options_pill_h();
        match self.phone_box_placed(Rect::new(0.0, 0.0, 1.0, ph)).last() {
            Some((last, _)) => last.y + last.h - ph + BOX_FOOT + self.phone_pill.scroll,
            None => 0.0,
        }
    }

    /// The tallest the box may be: what the bar's boxes are, under the
    /// pill.
    fn phone_box_max_h(&self) -> f32 {
        crate::options::BOX_DRAWER_H * self.options_scale() - self.options_pill_h()
    }

    /// The box's height now (it eases from one page's to the next's).
    fn phone_box_h(&self) -> f32 {
        // The Configure box grows and shrinks under the hand (Sync on adds
        // its parts, the phone's answer adds a line): the box is as tall as
        // it is NOW, not as it was when last eased — its Cancel and Apply,
        // at its foot, were cut off (Max, 2026-10-11).
        // ONE height for every page: the height every open box on this
        // bar grows to ("one drawer, one height"). A page taller than it
        // scrolls; a shorter one leaves the rest empty — and the box is
        // never taken from under the pointer.
        self.phone_box_max_h()
    }

    /// The Configure box shown in this box, if it is.
    fn phone_config(&self) -> Option<&crate::desktop_config::Config> {
        self.desktop.config.as_ref().filter(|c| c.hosted && self.phone_pill.page == Page::Configure)
    }

    /// The Configure box shown here is taller than the box: its foot
    /// (Cancel, Apply) is kept in sight at the box's bottom while the rest
    /// scrolls — where the foot begins in the Configure box, and how tall it
    /// is. (Scrolled out of sight, its owner found "no apply or cancel":
    /// Max, 2026-10-11.)
    fn phone_config_foot(&self) -> Option<(f32, f32)> {
        let config = self.phone_config()?;
        let height = config.built([1.0; 4], [0.0; 4]).2;
        if height <= self.phone_box_max_h() {
            return None;
        }
        let top = config.foot_top()?;
        Some((top, height - top))
    }

    /// What of the Configure box shown here is under `ptr` (`long`: the
    /// long pill and its box, whole): its foot where the foot is kept, the
    /// rest above it.
    fn phone_config_hit(&self, ptr: (f32, f32), long: Rect) -> Option<crate::desktop_config::Hit> {
        let config = self.phone_config()?;
        let Some((foot_top, foot_h)) = self.phone_config_foot() else {
            return config.hit(ptr);
        };
        let strip_top = long.y + long.h - foot_h;
        if ptr.1 >= strip_top {
            config.hit((ptr.0, ptr.1 - strip_top + config.rect.y + foot_top))
        } else {
            config.hit(ptr)
        }
    }

    /// Put the Configure box shown here away (nothing of it is kept: it
    /// applies only by its own Apply).
    fn phone_config_drop(&mut self) {
        if self.desktop.config.as_ref().is_some_and(|c| c.hosted) {
            self.desktop_config_edit(false);
            self.desktop.config = None;
            // (The desktop may hold the keys still, kept for the box.)
            self.desktop_pointer_went();
        }
    }

    /// Place the Configure box shown here under the long pill (called each
    /// frame of the pill's motion, and before it is pointed at).
    fn phone_config_place(&mut self) {
        if self.phone_pill.page != Page::Configure {
            return;
        }
        let Some(rect) = self.phone_pill_on_bar() else {
            return;
        };
        let long = self.phone_pill_long(rect);
        let ph = self.options_pill_h();
        if let Some(config) = self.desktop.config.as_mut().filter(|c| c.hosted) {
            let h = config.built([1.0; 4], [0.0; 4]).2;
            config.rect = Rect::new(long.x, long.y + ph - self.phone_pill.scroll, long.w, h);
        }
    }

    /// Turn the box's page; what the page needs is fetched.
    fn phone_box_turn(&mut self, page: Page) {
        if page != Page::Configure {
            self.phone_config_drop();
        }
        self.phone_pill.page = page;
        self.phone_pill.scroll = 0.0;
        match page {
            Page::Configure => {
                if let Some(item) = self.phone_pill_volume().and_then(|(_, _, item)| item) {
                    self.desktop_open_config(item, (0.0, 0.0));
                    if let Some(config) = self.desktop.config.as_mut() {
                        config.hosted = true;
                        config.t = 1.0;
                    }
                    self.request_desktop_draw();
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
        }
        self.phone_pill_sync();
    }

    /// A scroll over the pill: down opens its box, up folds it (the task
    /// pill's gesture and its sign).
    pub(crate) fn phone_pill_axis(&mut self, value: f32) {
        if value.abs() < crate::options::SCROLL_DEADZONE {
            return;
        }
        // Open, on a page taller than the box: the wheel moves the page;
        // only at its top does a scroll up fold the box.
        if self.phone_pill.box_open {
            let over = (self.phone_box_page_h() - self.phone_box_max_h()).max(0.0);
            let to = (self.phone_pill.scroll - value * 1.6).clamp(0.0, over);
            if to != self.phone_pill.scroll {
                self.phone_pill.scroll = to;
                self.phone_config_place();
                self.draw_options();
                return;
            }
            if value < 0.0 {
                return;
            }
        }
        let open = value < 0.0 && self.phone_pill.shown.is_some();
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
        // Its menu: the row under the pointer (unless the keys have it and
        // the pointer has not moved).
        if self.phone_pill.page == Page::Menu {
            self.phone_menu_sync();
            let ptr = self.options_ptr.filter(|_| on);
            if self.phone_pill.menu.as_mut().is_some_and(|m| m.pointer(ptr)) {
                self.draw_options();
            }
        }
        // The Configure box shown here: what of it is under the pointer.
        if self.phone_pill.page == Page::Configure {
            self.phone_config_place();
            let ptr = self.options_ptr.filter(|_| on);
            let long = self.phone_pill_on_bar().map(|rect| self.phone_pill_long(rect));
            let hover = ptr.zip(long).and_then(|(p, long)| self.phone_config_hit(p, long));
            if let Some(config) = self.desktop.config.as_mut().filter(|c| c.hosted) {
                if hover != config.hover {
                    config.hover = hover;
                    self.draw_options();
                }
            }
        }
        on
    }

    /// Its long pill is out (it stands over what was there).
    pub(crate) fn phone_pill_out(&self) -> bool {
        self.phone_pill.open > 0.05
    }

    /// The pointer left the bar: the long pill goes back in.
    pub(crate) fn phone_pill_close(&mut self) {
        // (Its name is being typed: the visit is not over because the hand
        // went to the keyboard.)
        if self.desktop.config.as_ref().is_some_and(|c| c.hosted && c.editing) {
            return;
        }
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
            // The Configure box shown here: the click is its (as on the
            // desktop: a press anywhere but the name settles the name).
            // Applied or cancelled, it is gone and the menu is back.
            if self.phone_pill.page == Page::Configure {
                self.phone_config_place();
                let hit = self.phone_config_hit(ptr, self.phone_pill_long(rect));
                if hit != Some(crate::desktop_config::Hit::Name) {
                    self.desktop_config_edit(false);
                }
                if let Some(hit) = hit {
                    self.desktop_config_act(hit);
                }
                // (What was clicked may have made it taller or shorter.)
                let over = (self.phone_box_page_h() - self.phone_box_max_h()).max(0.0);
                self.phone_pill.scroll = self.phone_pill.scroll.min(over);
                self.phone_config_place();
                self.sync_options_input();
                self.schedule_phone_pill_frame();
                if self.desktop.config.is_none() {
                    self.phone_box_turn(Page::Menu);
                }
                self.phone_pill_sync();
                return;
            }
            let long = self.phone_pill_long(rect);
            // Its menu: the row under the pointer.
            if self.phone_pill.page == Page::Menu {
                self.phone_menu_sync();
                let action = self.phone_pill.menu.as_ref().and_then(|m| m.hit(ptr).and_then(|row| m.action(row)));
                if let Some(action) = action {
                    tracing::info!("desktop: {action:?} from the bar's phone pill");
                    self.phone_box_act(menu_act(action), (long.x, round.y + round.h + 8.0));
                }
                return;
            }
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
            // (A phone whose USB debugging is off: its menu's help rows.)
            "fakehelp" => {
                self.phone_pill.fake = Some(Shown {
                    path: "/nowhere/a-phone".to_owned(),
                    says: "Max's Pixel · Turn on USB debugging".to_owned(),
                    battery: Some(66),
                    button: Some(HOW),
                    debugging: Debugging::Off,
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
            // The wheel over the open box: `scroll <px>` down, negative up.
            "scroll" => {
                let by: f32 = rest.trim().parse().unwrap_or(0.0);
                self.phone_pill_axis(-by / 1.6);
            }
            // What the box's page lists.
            "lines" => {
                let lines: Vec<String> = self
                    .phone_box_lines()
                    .into_iter()
                    .map(|l| if l.value.is_empty() { l.label } else { format!("{} = {}", l.label, l.value) })
                    .collect();
                return format!("{:?}: {lines:?}", self.phone_pill.page);
            }
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
            // A key on the open box, as the keyboard would give it (`key
            // down|up|home|end|enter|left|right|escape|<letter>`).
            "key" => {
                let name = rest.trim();
                let key = match name {
                    "down" => Keysym::Down,
                    "up" => Keysym::Up,
                    "home" => Keysym::Home,
                    "end" => Keysym::End,
                    "enter" => Keysym::Return,
                    "left" => Keysym::Left,
                    "right" => Keysym::Right,
                    "escape" => Keysym::Escape,
                    _ => Keysym::NoSymbol,
                };
                let letter = (key == Keysym::NoSymbol).then_some(name);
                self.phone_menu_sync();
                if !self.phone_box_keys() {
                    return "the box does not hold the keyboard".to_owned();
                }
                self.phone_box_key(key, letter);
                let hover = self.phone_pill.menu.as_ref().and_then(|m| m.hover.and_then(|i| m.rows.get(i).cloned()));
                return format!("key {name}: {:?} on {hover:?}", self.phone_pill.page);
            }
            "close" => self.phone_pill.forced = Some(false),
            "free" => self.phone_pill.forced = None,
            _ => return "phonepill fake [words]|fakehelp|gone|open|close|free|box|fold|menu|camera|configure|properties|line <n>|key <name>".to_owned(),
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
            // The phone's menu: drawn by the one menu's painter, its rows on
            // the box's own panel (symbols, the quiet tint, the red). The
            // other pages: a line each, the one under the pointer on the
            // hover wash, a line drawn once the opening box has room for all
            // of it.
            if let (true, Some(menu)) = (e > 0.01, self.phone_pill.menu.as_ref().filter(|_| p.page == Page::Menu)) {
                let paint = MenuPaint {
                    fill: [panel[0], panel[1], panel[2], self.box_panel_alpha() * solid * out],
                    ink: [box_ink[0], box_ink[1], box_ink[2], box_ink[3] * out],
                    wash: self.options_hover_wash(),
                    radius: crate::clipboard::BOX_RADIUS,
                    glass: 0.0,
                    accent: MenuPaint::accent_now(),
                };
                let glyph = |name: &str| p.glyph_layer.get(&crate::glyphs::key(name)).copied();
                menu.push_rows(scene, &paint, &glyph);
            } else if e > 0.01 {
                let hover = self.options_hover_wash();
                for (row, l) in self.phone_box_placed(long) {
                    // (Scrolled up past the pill, or not yet in the opening
                    // box: not drawn.)
                    if row.y < long.y + ph - 0.5 {
                        continue;
                    }
                    if row.y + row.h > long.y + long.h + 0.5 {
                        break;
                    }
                    if l.act != Act::Nothing && ptr.is_some_and(|p| row.contains(p)) {
                        scene.rects.push(RectInst {
                            rect: Rect::new(row.x + 6.0, row.y, row.w - 12.0, row.h),
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
                        let chip = Rect::new(row.x + BOX_PAD, row.y + (row.h - d) / 2.0, d, d);
                        // (Quiet, 2026-10-16: the step's number in the
                        // accent, nothing behind it.)
                        let amber = crate::notif::amber();
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
                            color: Some([amber[0], amber[1], amber[2], solid * out]),
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
                    word(l.label, row.x + BOX_PAD + indent, if l.sub || l.dim { 0.6 } else { 1.0 });
                    if !l.value.is_empty() {
                        // (Set from the right, by its measured width.)
                        let w = p.value_w.get(&l.value).copied().unwrap_or(l.value.chars().count() as f32 * FONT_PX * s * 0.56);
                        word(l.value, row.x + row.w - BOX_PAD - w, 0.7);
                    }
                }
            }
            // The Configure box, in place of lines: all it is made of, as it
            // places it itself — what the opening box has room for.
            if let Some(config) = self.phone_config() {
                let alpha = solid * out;
                let wash = self.options_rest_wash();
                let (rects, labels, _) = config.built(
                    [ink[0], ink[1], ink[2], ink[3] * alpha],
                    [wash[0], wash[1], wash[2], wash[3] * alpha],
                );
                // (Cut to the box under the pill, as the bar's other boxes
                // cut their lists: a part half scrolled away is half drawn,
                // not a knob left standing without its switch.)
                let foot = self.phone_config_foot();
                let foot_h = foot.map_or(0.0, |(_, h)| h.min(long.h - ph));
                let clip = Rect::new(long.x, long.y + ph, long.w, (long.h - ph - foot_h).max(0.0));
                // (Each word keeps to its own clip AND the box's.)
                let within = |r: Rect, clip: Rect| {
                    let (x0, y0) = (r.x.max(clip.x), r.y.max(clip.y));
                    let (x1, y1) = ((r.x + r.w).min(clip.x + clip.w), (r.y + r.h).min(clip.y + clip.h));
                    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
                };
                let cut = |labels: &[Label], dy: f32, clip: Rect| -> Vec<Label> {
                    labels
                        .iter()
                        .filter_map(|l| {
                            let mut l = l.clone();
                            let own = l.clip.unwrap_or(Rect::new(l.pos.0, l.pos.1, l.max_w, l.line_px));
                            l.pos.1 += dy;
                            l.clip = Some(within(Rect::new(own.x, own.y + dy, own.w, own.h), clip)?);
                            Some(l)
                        })
                        .collect()
                };
                // Its foot, kept at the box's bottom: the same things, moved
                // there, cut to that strip.
                if let Some((foot_top, _)) = foot {
                    let strip = Rect::new(long.x, long.y + long.h - foot_h, long.w, foot_h);
                    let dy = strip.y - (config.rect.y + foot_top);
                    let moved: Vec<RectInst> =
                        rects.iter().map(|r| RectInst { rect: Rect::new(r.rect.x, r.rect.y + dy, r.rect.w, r.rect.h), ..*r }).collect();
                    scene.grids.push(crate::content::GridContent {
                        clip: strip,
                        rects: moved,
                        labels: cut(&labels, dy, strip),
                        ..Default::default()
                    });
                }
                let labels = cut(&labels, 0.0, clip);
                scene.grids.push(crate::content::GridContent { clip, rects, labels, ..Default::default() });
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
                let amber = crate::notif::amber();
                // (Quiet: the word in the accent; its tint under the pointer.)
                if lit {
                    scene.rects.push(RectInst {
                        rect: chip,
                        radius: chip.h / 2.0,
                        color: [amber[0], amber[1], amber[2], 0.3 * out],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                let color = if lit { color } else { Some([amber[0], amber[1], amber[2], out]) };
                scene.labels.push(Label {
                    text: tr(word).to_owned(),
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

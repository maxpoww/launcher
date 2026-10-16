//! The control panel's settings beyond the screen: what each one shows when
//! its pill is opened, and what a click on one of its choices does.
//!
//! Every setting is drawn with the idiom Scale and Resolution set
//! ([`crate::panel`], 2026-10-01): a subtitle, then groups of floating choice
//! pills, the one in use lit. This module owns the other half — reading the
//! machine into a [`ControlView`] and carrying out a pick — on its own
//! thread, because most of it asks other programs (`wpctl`, `nmcli`,
//! `powerprofilesctl`, `gsettings`…) and none of that may hold up a frame.
//!
//! The same three steps as the display settings wherever the compositor is
//! involved: change it live (`hl.config` over the control socket), save the
//! choice in `~/.config/golem/settings.json`, and have it in the generated
//! `settings.lua` so it is there after a restart ([`crate::display`]).
//!
//! A few choices belong to the UI thread (the night light's warmth, do not
//! disturb, opening a page of the gear box): [`ui_key`] names them, `App`
//! runs them itself and tells the worker only to read again.

use std::process::{Command, Stdio};
use std::sync::mpsc;

use calloop::channel::Sender;
use tracing::{debug, warn};

use crate::display::GolemSettings;
use crate::i18n::tr;

/// One choice of a setting.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Choice {
    pub text: String,
    /// The one in use.
    pub on: bool,
    /// What a pick of it sends back (with its group's key).
    pub value: String,
    /// A PALETTE: the colours its swatch is painted with (ground first, then
    /// its soul), drawn in place of a picture. Empty for every other choice.
    pub swatch: Vec<[u8; 3]>,
}

/// A group of choices under an optional caption. A caption with no choices
/// is a line of text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Group {
    pub caption: Option<String>,
    /// What the group sets: see [`apply`].
    pub key: &'static str,
    /// A choice pill's width, in the panel's design pixels.
    pub width: f32,
    pub choices: Vec<Choice>,
    /// The choices are PICTURES (each one's `value` is its file): shown as
    /// a grid of previews, as big as the box lets them all be.
    pub tiles: bool,
    /// The choices are the stops of a SLIDER (0..=100 by `SLIDER_STEP`),
    /// drawn as a track with a knob on the lit one.
    pub slider: bool,
    /// Laid on the same row as the group before it (a slider's own
    /// [Dark][Light] pair), at its right end.
    pub beside: bool,
    /// The choices are COLOUR CELLS (their `swatch`), this many to a row:
    /// the palette editor's field and hue strip. 0 = not cells.
    pub cells: usize,
}

/// THE PALETTE EDITOR (Max, 2026-10-16; the mockup's "+" tile): six places
/// — Accent, Ground, Panels, Web, Text, Detail — each given a colour from a
/// field of the hue chosen on the strip under it. Saved as the owner's own.
#[derive(Debug, Clone)]
pub(crate) struct Editor {
    pub soul: [[u8; 3]; 6],
    /// The place being given its colour.
    pub cur: usize,
    /// The hue of the field, in degrees.
    pub hue: f32,
}

pub(crate) static EDITOR: std::sync::Mutex<Option<Editor>> = std::sync::Mutex::new(None);

/// The editor's field: chroma across, lightness down.
const FIELD_COLS: usize = 18;
const FIELD_ROWS: usize = 7;
const HUES: usize = 24;

fn field_colour(col: usize, row: usize, hue: f32) -> [u8; 3] {
    let c = 0.22 * col as f32 / (FIELD_COLS - 1) as f32;
    let l = 0.96 - 0.84 * row as f32 / (FIELD_ROWS - 1) as f32;
    crate::palette::tint::from_lch(l, c, hue)
}

/// Open the editor on the palette in use (its six as the start).
pub(crate) fn editor_open() {
    let pick = crate::palette::choice();
    let id = pick.palette_id().to_owned();
    let soul = crate::palette::all()
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| {
            // Its six in the editor's places: accent, ground, panels, web, text, detail.
            [p.dark[p.accent], p.dark[1], p.dark[2], p.dark[1], p.dark[5], p.dark[3]]
        })
        .unwrap_or([[0xff, 0xbe, 0x98], [0x1d, 0x20, 0x26], [0x2a, 0x2e, 0x36], [0x1d, 0x20, 0x26], [0xe6, 0xe6, 0xe0], [0x44, 0x48, 0x50]]);
    let hue = crate::palette::tint::lch(soul[0]).2;
    if let Ok(mut g) = EDITOR.lock() {
        *g = Some(Editor { soul, cur: 0, hue });
    }
}

/// The palette the editor holds now (for its live preview).
pub(crate) fn editor_draft() -> Option<crate::palette::Palette> {
    let e = EDITOR.lock().ok()?.clone()?;
    Some(crate::palette_gen::direct(crate::palette::DRAFT_ID, tr("New palette"), e.soul))
}

/// A pick in the editor (`key`, `value`); true when it was the editor's.
/// Save makes the palette and hands its id back through `saved`.
pub(crate) fn editor_pick(key: &str, value: &str, saved: &mut Option<String>) -> bool {
    let Ok(mut g) = EDITOR.lock() else { return false };
    match key {
        "colors-edit-role" => {
            if let (Some(e), Ok(i)) = (g.as_mut(), value.parse::<usize>()) {
                e.cur = i.min(5);
                e.hue = crate::palette::tint::lch(e.soul[e.cur]).2;
            }
        }
        "colors-edit-hue" => {
            if let (Some(e), Ok(h)) = (g.as_mut(), value.parse::<f32>()) {
                e.hue = h;
            }
        }
        "colors-edit-colour" => {
            if let (Some(e), Some(c)) = (g.as_mut(), crate::palette::parse_hex(value)) {
                e.soul[e.cur] = c;
            }
        }
        "colors-edit" => match value {
            "save" => {
                if let Some(e) = g.take() {
                    let n = (1..100).find(|n| !crate::palette::all().iter().any(|p| p.id == format!("mine-{n}"))).unwrap_or(1);
                    let id = format!("mine-{n}");
                    let name = crate::i18n::fill(tr("My palette {n}"), &[("n", &n.to_string())]);
                    let p = crate::palette_gen::direct(&id, &name, e.soul);
                    if crate::palette::save_own(&p).is_ok() {
                        *saved = Some(id);
                    }
                }
            }
            _ => *g = None,
        },
        _ => return false,
    }
    true
}

/// What an open setting shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ControlView {
    pub label: &'static str,
    pub subtitle: String,
    pub groups: Vec<Group>,
}

/// The worker's own door, for a thread of its that finishes later (the
/// wallpaper's): what it has to save goes back through the worker, which
/// is the one that writes the settings — two threads writing the file each
/// from its own reading of it would drop one another's change.
static TO_WORKER: std::sync::OnceLock<std::sync::Mutex<mpsc::Sender<ControlCommand>>> = std::sync::OnceLock::new();

/// The wallpaper picked and not yet up and saved: the box shows IT as the
/// one in use meanwhile (the worker reads the setting again right after a
/// pick, when the file still names the old one — the mark went back to the
/// old tile and stayed there until the box was opened again).
static WALLPAPER_WANT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// One wallpaper is put up at a time — a pick's or the start's: two
/// hand-overs at once would each leave a player up.
static WALLPAPER_ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What the UI thread knows that the worker cannot ask anyone for.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct UiFacts {
    /// Do not disturb is on.
    pub dnd: bool,
    /// The screen's warmth in kelvin (6500 = the night light off).
    pub night_k: u32,
    /// The update line the gear box shows ("Up to date"…).
    pub updates: String,
    /// The gear box has these pages on this machine.
    pub has_bt: bool,
    pub has_battery: bool,
    /// The apps in the grid, most used first: (desktop id, name).
    pub apps: Vec<(String, String)>,
}

/// The Colours page shows its APPS LIST (the "+" beside Apps) instead.
pub(crate) static APPS_LIST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How many apps the list shows (the most used): the panel does not scroll.
const APPS_SHOWN: usize = 36;

#[derive(Debug)]
pub(crate) enum ControlCommand {
    /// Read the setting `label` and send its view.
    Read(&'static str, UiFacts),
    /// Carry out a pick, then read the setting again.
    Set { label: &'static str, key: &'static str, value: String, ui: UiFacts },
}

#[derive(Debug)]
pub(crate) enum ControlEvent {
    View(ControlView),
    /// The shell's language changed: load its strings again.
    Language,
    /// The time zone changed: the shell's own clocks follow.
    Zone(String),
}

pub(crate) struct ControlsHandle {
    tx: mpsc::Sender<ControlCommand>,
}

impl ControlsHandle {
    pub(crate) fn send(&self, cmd: ControlCommand) {
        if let Err(e) = self.tx.send(cmd) {
            warn!("controls: worker gone, dropping command: {e}");
        }
    }
}

/// Start the worker. It sleeps until asked: nothing is polled.
pub(crate) fn spawn(events: Sender<ControlEvent>) -> ControlsHandle {
    let (tx, rx) = mpsc::channel::<ControlCommand>();
    let _ = TO_WORKER.set(std::sync::Mutex::new(tx.clone()));
    let spawned = std::thread::Builder::new().name("controls".into()).spawn(move || {
        let mut zone = ZonePick::default();
        while let Ok(cmd) = rx.recv() {
            let (label, ui) = match cmd {
                ControlCommand::Read(label, ui) => (label, ui),
                ControlCommand::Set { label, key, value, ui } => {
                    let told = match apply(key, &value, &mut zone) {
                        Applied::Done => Ok(()),
                        Applied::Language => events.send(ControlEvent::Language),
                        Applied::Zone(z) => events.send(ControlEvent::Zone(z)),
                    };
                    if told.is_err() {
                        break;
                    }
                    (label, ui)
                }
            };
            if events.send(ControlEvent::View(read(label, &ui, &mut zone))).is_err() {
                break;
            }
        }
    });
    if let Err(e) = spawned {
        warn!("controls: no worker thread ({e}); the settings stay empty");
    }
    ControlsHandle { tx }
}

/// The settings this module gives real content. (Scale and Resolution are
/// [`crate::display`]'s.)
pub(crate) fn is_control(label: &str) -> bool {
    matches!(
        label,
        "Wi-Fi"
            | "Bluetooth"
            | "Sound output"
            | "Volume"
            | "Microphone"
            | "Brightness"
            | "Night light"
            | "Notifications"
            | "Keyboard layout"
            | "Power mode"
            | "Touchpad speed"
            | "Natural scrolling"
            | "Updates"
            | "About Golem"
            | "Battery"
            | "Privacy"
            | "Time zone"
            | "Language"
            | "Default apps"
            | "Wallpaper"
            | "Colours"
            | "Tiles"
    )
}

/// The keys `App` carries out itself (see the module docs).
pub(crate) fn ui_key(key: &str) -> bool {
    matches!(
        key,
        "open" | "night" | "dnd" | "restart" | "colors-edit" | "colors-edit-role" | "colors-edit-colour" | "colors-edit-hue" | "colors-apps-list" | "colors-skip" | "colors-apps" | "colors-palette" | "colors-web" | "colors-icons"
            | "colors-apps-look" | "colors-web-look" | "colors-icons-look"
    )
}

// ---------------------------------------------------------------- helpers

/// The longest a program asked for a reading or a change may take. One that
/// hangs (a bus that never answers) is ended: the worker must come back.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(4);

/// Run a program to its end, or end it at [`PATIENCE`]. `None`: it is not
/// there, was ended, or failed.
fn finish(cmd: Command, keep_output: bool) -> Option<String> {
    finish_within(cmd, keep_output, PATIENCE)
}

/// [`finish`] with a patience of its own, for the few programs whose work
/// is long by nature.
fn finish_within(mut cmd: Command, keep_output: bool, patience: std::time::Duration) -> Option<String> {
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    cmd.stdout(if keep_output { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().ok()?;
    // The output is read on its own thread: a full pipe would stop the
    // program before it could end.
    let reader = child.stdout.take().map(|mut o| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut text = String::new();
            let _ = o.read_to_string(&mut text);
            text
        })
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() < patience => std::thread::sleep(std::time::Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    match status {
        Some(st) if st.success() => Some(text),
        Some(st) => {
            debug!("controls: a program ended {st}");
            None
        }
        None => {
            warn!("controls: a program did not answer in {patience:?}; ended");
            None
        }
    }
}

/// A program's standard output, or `None` when it is missing or failed.
fn out(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    finish(cmd, true)
}

/// Run a program for its effect.
fn run(program: &str, args: &[&str]) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(args);
    let ok = finish(cmd, false).is_some();
    if !ok {
        debug!("controls: {program} {args:?} did not succeed");
    }
    ok
}

fn choice(text: impl Into<String>, on: bool, value: impl Into<String>) -> Choice {
    Choice { text: text.into(), on, value: value.into(), swatch: Vec::new() }
}

fn group(caption: Option<&'static str>, key: &'static str, width: f32, choices: Vec<Choice>) -> Group {
    // A pill is as wide as its group's longest word needs: the widths given
    // are for the English, and another language's words run longer.
    let longest = choices.iter().map(|c| c.text.chars().count()).max().unwrap_or(0) as f32;
    let width = width.max(longest * LETTER + 2.0 * PILL_AIR).min(WIDEST_PILL);
    Group { caption: caption.map(|c| tr(c).to_owned()), key, width, choices, tiles: false, slider: false, beside: false, cells: 0 }
}

/// A letter of a choice pill's text, on average, and the air at each end,
/// in the panel's design pixels; and the widest a pill may grow.
const LETTER: f32 = 8.4;
const PILL_AIR: f32 = 16.0;
const WIDEST_PILL: f32 = 260.0;

/// A line of text.
fn line(text: String) -> Group {
    Group { caption: Some(text), key: "", width: 0.0, choices: Vec::new(), tiles: false, slider: false, beside: false, cells: 0 }
}

/// A slider's stops, and where the palette AS MADE sits on it.
pub(crate) const SLIDER_STEP: u8 = 5;
const MADE: u8 = 50;

/// A slider from 0 to 100, the stop nearest `level` lit.
fn slider(key: &'static str, level: u8) -> Group {
    let lit = (level + SLIDER_STEP / 2) / SLIDER_STEP;
    let choices = (0..=100 / SLIDER_STEP).map(|i| choice(String::new(), i == lit, (i * SLIDER_STEP).to_string())).collect();
    Group { slider: true, ..group(None, key, 0.0, choices) }
}

/// What a slider's level is called: Off, as made, full, or its number.
fn level_text(level: u8) -> String {
    match level {
        0 => tr("Off").to_owned(),
        MADE => tr("as made").to_owned(),
        100 => tr("full colour").to_owned(),
        n => format!("{n}%"),
    }
}

/// [Dark][Light], with the values each sends.
fn look_pair(caption: Option<&'static str>, key: &'static str, dark: bool, dark_value: &'static str, light_value: &'static str) -> Group {
    group(caption, key, 84.0, vec![choice(tr("Dark"), dark, dark_value), choice(tr("Light"), !dark, light_value)])
}

/// On / Off.
fn on_off(caption: Option<&'static str>, key: &'static str, on: bool) -> Group {
    group(caption, key, 96.0, vec![choice(tr("On"), on, "on"), choice(tr("Off"), !on, "off")])
}

/// The way into a page of the gear box.
fn more(page: &'static str) -> Group {
    group(None, "open", 150.0, vec![choice(tr("More…"), false, page)])
}

/// The stop of `stops` nearest `value`.
fn nearest(stops: &[f64], value: f64) -> usize {
    stops
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - value).abs().total_cmp(&(b.1 - value).abs()))
        .map_or(0, |(i, _)| i)
}

// ------------------------------------------------------------------ sound

/// A sound device PipeWire knows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SoundNode {
    pub id: u64,
    pub name: String,
    pub description: String,
}

/// The devices of one class (`Audio/Sink`, `Audio/Source`) in a `pw-dump`,
/// and the name of the default one.
pub(crate) fn parse_sound(dump: &serde_json::Value, class: &str) -> (Vec<SoundNode>, Option<String>) {
    let Some(objects) = dump.as_array() else {
        return (Vec::new(), None);
    };
    let mut nodes = Vec::new();
    let mut default = None;
    let key = if class == "Audio/Sink" { "default.audio.sink" } else { "default.audio.source" };
    for o in objects {
        let ty = o["type"].as_str().unwrap_or_default();
        if ty.ends_with("Node") {
            let props = &o["info"]["props"];
            if props["media.class"].as_str() != Some(class) {
                continue;
            }
            let Some(id) = o["id"].as_u64() else { continue };
            let name = props["node.name"].as_str().unwrap_or_default().to_owned();
            let description = props["node.description"]
                .as_str()
                .or_else(|| props["node.nick"].as_str())
                .unwrap_or(&name)
                .to_owned();
            nodes.push(SoundNode { id, name, description });
        } else if ty.ends_with("Metadata") && o["props"]["metadata.name"].as_str() == Some("default") {
            for m in o["metadata"].as_array().into_iter().flatten() {
                if m["key"].as_str() == Some(key) {
                    default = m["value"]["name"].as_str().map(str::to_owned);
                }
            }
        }
    }
    nodes.sort_by(|a, b| a.description.cmp(&b.description));
    (nodes, default)
}

/// `wpctl get-volume`'s line as (level 0..1+, muted).
pub(crate) fn parse_volume(text: &str) -> Option<(f64, bool)> {
    let rest = text.trim().strip_prefix("Volume:")?.trim();
    let level = rest.split_whitespace().next()?.parse::<f64>().ok()?;
    Some((level, rest.contains("MUTED")))
}

fn sound_dump() -> serde_json::Value {
    out("pw-dump", &[]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null)
}

/// A name short enough for a choice pill.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

const LEVELS: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// The level choices for a sink or a source (`target`: wpctl's name for it).
fn level_group(caption: &'static str, key: &'static str, target: &str) -> Option<(Group, Group, f64, bool)> {
    let (level, muted) = parse_volume(&out("wpctl", &["get-volume", target])?)?;
    let lit = nearest(&LEVELS, level);
    let levels = LEVELS
        .iter()
        .enumerate()
        .map(|(i, l)| choice(format!("{:.0}%", l * 100.0), i == lit && !muted, format!("{l}")))
        .collect();
    let mute_key = if key == "volume" { "mute" } else { "mic-mute" };
    let mute = group(None, mute_key, 110.0, vec![choice(tr("Sound on"), !muted, "off"), choice(tr("Muted"), muted, "on")]);
    Some((group(Some(caption), key, 76.0, levels), mute, level, muted))
}

fn read_devices(view: &mut ControlView, class: &str, key: &'static str, none: &'static str) {
    let (nodes, default) = parse_sound(&sound_dump(), class);
    if nodes.is_empty() {
        view.subtitle = tr(none).to_owned();
        return;
    }
    let lit = nodes.iter().find(|n| Some(&n.name) == default.as_ref());
    view.subtitle = lit.map_or_else(|| tr("Nothing chosen").to_owned(), |n| n.description.clone());
    let choices = nodes
        .iter()
        .map(|n| choice(short(&n.description, 24), Some(&n.name) == default.as_ref(), n.id.to_string()))
        .collect();
    view.groups.push(group(None, key, 230.0, choices));
}

// ---------------------------------------------------------------- network

fn wifi_state() -> Option<(bool, Option<String>)> {
    let on = out("nmcli", &["-t", "-f", "WIFI", "radio"])?.trim() == "enabled";
    let name = out("nmcli", &["-t", "-f", "ACTIVE,SSID", "dev", "wifi"]).and_then(|t| {
        t.lines().find_map(|l| l.strip_prefix("yes:").map(str::to_owned)).filter(|s| !s.is_empty())
    });
    Some((on, name))
}

/// Whether the Bluetooth adapter is on (`None`: this machine has none).
fn bt_powered() -> Option<bool> {
    let show = out("bluetoothctl", &["show"])?;
    show.lines().find_map(|l| l.trim().strip_prefix("Powered:")).map(|v| v.trim() == "yes")
}

// ------------------------------------------------------------- brightness

const BRIGHT: [f64; 5] = [0.1, 0.25, 0.5, 0.75, 1.0];

/// The screen's backlight as 0..1 (`None`: no backlight to set — a desktop
/// monitor, or a machine without `golem-brightness`). Read from the
/// helper's machine line (`device,class,current,NN%,max`), whose percentage
/// is on the same curve its `set NN%` uses.
fn brightness() -> Option<f64> {
    parse_brightness(&out("golem-brightness", &["-m"])?)
}

pub(crate) fn parse_brightness(line: &str) -> Option<f64> {
    let pct = line.trim().split(',').nth(3)?.trim_end_matches('%').parse::<f64>().ok()?;
    Some((pct / 100.0).clamp(0.0, 1.0))
}

// ----------------------------------------------------------------- inputs

/// The keyboard layouts offered: (name as its speakers write it, xkb layout,
/// xkb variant). The installer's list (`system/hardware/keyboards.nix`) is
/// the long one; these are the ones a panel of pills can hold. The layout in
/// use is always shown, whether it is here or not.
pub(crate) const LAYOUTS: [(&str, &str, &str); 15] = [
    ("English (US)", "us", ""),
    ("US international", "us", "intl"),
    ("English (UK)", "gb", ""),
    ("Español", "es", ""),
    ("Español (Latam)", "latam", ""),
    ("Português", "pt", ""),
    ("Português (BR)", "br", ""),
    ("Français", "fr", ""),
    ("Deutsch", "de", ""),
    ("Italiano", "it", ""),
    ("Nederlands", "nl", ""),
    ("Polski", "pl", ""),
    ("Svenska", "se", ""),
    ("Dansk", "dk", ""),
    ("Türkçe", "tr", ""),
];

/// A layout or variant name the compositor can be handed: xkb's own names
/// are lower-case letters, digits and a little punctuation.
pub(crate) fn valid_xkb(text: &str) -> bool {
    text.len() <= 40 && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ',' | '(' | ')'))
}

/// xkb options: names like a layout's, with the colon of `grp:toggle`.
pub(crate) fn valid_xkb_options(text: &str) -> bool {
    text.len() <= 120 && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ',' | ':'))
}

/// The keyboard layout in use: the saved choice, else the compositor's.
fn keyboard_now() -> (String, String) {
    if let Some(k) = GolemSettings::load().keyboard {
        return (k.layout, k.variant);
    }
    let get = |name: &str| {
        crate::hypr::option_text(name)
            .map(|s| s.split(',').next().unwrap_or_default().trim().to_owned())
            // The compositor's word for an empty string.
            .filter(|s| s != "[[EMPTY]]")
    };
    (get("input:kb_layout").unwrap_or_else(|| "us".into()), get("input:kb_variant").unwrap_or_default())
}

const SPEEDS: [(f64, &str); 5] =
    [(-0.6, "Slowest"), (-0.3, "Slow"), (0.0, "Normal"), (0.3, "Fast"), (0.6, "Fastest")];

// ------------------------------------------------------------------- look

/// Whether the apps are asked for their dark look.
pub(crate) fn dark_now() -> bool {
    if let Some(dark) = GolemSettings::load().dark {
        return dark;
    }
    // Golem ships dark; a machine that never chose is dark.
    out("dconf", &["read", "/org/gnome/desktop/interface/color-scheme"]).is_none_or(|v| !v.contains("light"))
}

/// Ask the apps for a look. GTK 4 / libadwaita apps and the browser follow
/// at once (through the portal); GTK 3 apps at their next start.
///
/// Written with `dconf`, not `gsettings`: Golem's session carries no GSettings
/// schemas on its data path (`gsettings` answers "No schemas installed" —
/// found in the desktop VM, 2026-10-13), and dconf, where the values live,
/// needs none.
pub(crate) fn assert_dark(dark: bool) {
    let scheme = if dark { "'prefer-dark'" } else { "'prefer-light'" };
    let theme = if dark { "'Adwaita-dark'" } else { "'Adwaita'" };
    run("dconf", &["write", "/org/gnome/desktop/interface/color-scheme", scheme]);
    // With adw-gtk3 there, GTK 3's theme is the palette's (`palette::gtk3_live`,
    // which follows this change); without it, Adwaita in the look's tone.
    if crate::palette::adw_gtk3(dark).is_none() {
        run("dconf", &["write", "/org/gnome/desktop/interface/gtk-theme", theme]);
    }
    // Golem's icons in the same tone (Max, 2026-10-14): silver `Golem` in the
    // light, graphite `Golem-Dark` in the dark — where they are installed.
    if crate::look::installed("Golem") {
        let icons = if dark { "'Golem-Dark'" } else { "'Golem'" };
        run("dconf", &["write", "/org/gnome/desktop/interface/icon-theme", icons]);
    }
}

// ------------------------------------------------------------------ power

fn battery_line() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for d in dir.flatten() {
        let p = d.path();
        if std::fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "Battery")
            && !p.join("scope").exists()
        {
            let pct = std::fs::read_to_string(p.join("capacity")).ok()?.trim().to_owned();
            let status = std::fs::read_to_string(p.join("status")).unwrap_or_default();
            let what = match status.trim() {
                "Charging" => tr("charging"),
                "Full" | "Not charging" => tr("plugged in"),
                _ => tr("on battery"),
            };
            return Some(format!("{pct}% · {what}"));
        }
    }
    None
}

// ------------------------------------------------------------------ about

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "Golem".into())
}

fn machine_lines() -> Vec<String> {
    let mut lines = Vec::new();
    let dmi = |f: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{f}")).ok().map(|s| s.trim().to_owned());
    if let Some(model) = dmi("product_name").filter(|m| !m.is_empty() && m != "System Product Name") {
        let vendor = dmi("sys_vendor").unwrap_or_default();
        lines.push(if vendor.is_empty() || model.starts_with(&vendor) { model } else { format!("{vendor} {model}") });
    }
    if let Some(cpu) = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("model name")).map(|v| v.trim_start_matches([' ', '\t', ':']).to_owned()))
    {
        lines.push(crate::sys::clean_cpu_name(&cpu));
    }
    if let Some(kb) = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| {
        t.lines().find_map(|l| l.strip_prefix("MemTotal:")).and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
    }) {
        lines.push(tr("{n} GB of memory").replace("{n}", &format!("{:.0}", (kb / 1_048_576.0).ceil())));
    }
    lines
}

// ---------------------------------------------------------------- privacy

/// The apps recording from the microphone now, out of a `pw-dump`.
pub(crate) fn parse_recording(dump: &serde_json::Value) -> Vec<String> {
    let mut apps: Vec<String> = dump
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let props = &o["info"]["props"];
            (props["media.class"].as_str() == Some("Stream/Input/Audio")
                && o["info"]["state"].as_str() == Some("running"))
            .then(|| {
                props["application.name"]
                    .as_str()
                    .or_else(|| props["node.name"].as_str())
                    .unwrap_or(tr("An app"))
                    .to_owned()
            })
        })
        // The shell's own listeners (the music bars) are not someone recording.
        .filter(|n| !matches!(n.as_str(), "cava" | "waverunner"))
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

/// Whether some program holds a camera open.
fn camera_in_use() -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return false;
    };
    procs.flatten().any(|p| {
        std::fs::read_dir(p.path().join("fd")).is_ok_and(|fds| {
            fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.to_string_lossy().starts_with("/dev/video")))
        })
    })
}

// -------------------------------------------------------------- time zone

/// Where the owner is in choosing a time zone: a region, then the hour the
/// clock should show, then the place.
#[derive(Debug, Default)]
pub(crate) struct ZonePick {
    region: Option<String>,
    /// Minutes east of UTC.
    offset: Option<i32>,
    /// Which screenful of places is showing.
    page: usize,
    /// Every zone with its offset now, read once.
    zones: Option<Vec<(String, i32)>>,
}

/// tzdata's directory.
fn zoneinfo() -> Option<std::path::PathBuf> {
    std::env::var_os("TZDIR")
        .map(std::path::PathBuf::from)
        .into_iter()
        .chain(["/etc/zoneinfo".into(), "/usr/share/zoneinfo".into()])
        .find(|p| p.join("UTC").exists() || p.join("Etc/UTC").exists())
}

/// `Region/Place` names under a zoneinfo directory.
fn zone_names(dir: &std::path::Path) -> Vec<String> {
    // tzdata's own list of the zones that are places today (one a
    // territory, none of the old spellings kept for compatibility).
    if let Ok(tab) = std::fs::read_to_string(dir.join("zone1970.tab")) {
        let mut names = zone_tab_names(&tab);
        if !names.is_empty() {
            names.sort();
            return names;
        }
    }
    zone_names_walked(dir)
}

/// The zone names of a `zone1970.tab` (its third column).
pub(crate) fn zone_tab_names(tab: &str) -> Vec<String> {
    tab.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').nth(2))
        .filter(|z| z.contains('/'))
        .map(str::to_owned)
        .collect()
}

/// Every `Region/Place` file under a zoneinfo directory (no list there).
fn zone_names_walked(dir: &std::path::Path) -> Vec<String> {
    const REGIONS: [&str; 10] =
        ["Africa", "America", "Antarctica", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific", "Arctic"];
    let mut names = Vec::new();
    for region in REGIONS {
        let mut stack = vec![dir.join(region)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(dir) {
                    names.push(rel.to_string_lossy().into_owned());
                }
            }
        }
    }
    names.sort();
    names
}

/// `+0530` / `-0400` as minutes east of UTC.
pub(crate) fn parse_offset(text: &str) -> Option<i32> {
    let t = text.trim();
    let sign = match t.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = &t[1..];
    if digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m) = (digits[..2].parse::<i32>().ok()?, digits[2..].parse::<i32>().ok()?);
    Some(sign * (h * 60 + m))
}

/// Every zone with its offset from UTC right now (`date` knows the rules;
/// one shell runs it over the whole list).
fn zone_offsets() -> Vec<(String, i32)> {
    let Some(dir) = zoneinfo() else {
        return Vec::new();
    };
    let names = zone_names(&dir);
    let script = "while read -r z; do printf '%s %s\\n' \"$z\" \"$(TZ=\"$z\" date +%z)\"; done";
    let Ok(mut child) = Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(names.join("\n").as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let Ok(o) = child.wait_with_output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let (zone, off) = l.rsplit_once(' ')?;
            Some((zone.to_owned(), parse_offset(off)?))
        })
        .collect()
}

/// The zone the session is in: the saved choice, `$TZ`, else the system's.
fn zone_now() -> String {
    if let Some(z) = GolemSettings::load().time_zone {
        return z;
    }
    if let Some(z) = std::env::var("TZ").ok().filter(|z| z.contains('/')) {
        return z.trim_start_matches(':').to_owned();
    }
    system_zone().unwrap_or_else(|| "UTC".into())
}

/// The zone `/etc/localtime` points at.
fn system_zone() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let text = target.to_string_lossy();
    text.split_once("zoneinfo/").map(|(_, z)| z.to_owned())
}

/// `America/Argentina/Buenos_Aires` → `Buenos Aires`.
pub(crate) fn place_name(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

/// The clock `offset` minutes east of UTC shows when UTC shows `utc_min`
/// minutes past midnight.
pub(crate) fn clock_at(utc_min: i32, offset: i32) -> String {
    let m = (utc_min + offset).rem_euclid(24 * 60);
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn utc_minutes() -> i32 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    ((secs / 60) % (24 * 60)) as i32
}

/// The most places shown for one region and hour.
const MAX_PLACES: usize = 15;

fn read_zone(view: &mut ControlView, pick: &mut ZonePick) {
    let zones = pick.zones.get_or_insert_with(zone_offsets).clone();
    let now = zone_now();
    let now_offset = zones.iter().find(|(z, _)| *z == now).map(|(_, o)| *o);
    let utc = utc_minutes();
    view.subtitle = match now_offset {
        Some(o) => format!("{} · {}", place_name(&now), clock_at(utc, o)),
        None => place_name(&now),
    };
    if zones.is_empty() {
        return;
    }
    // One step at a time, each finished step folded to its one lit pill
    // (a click on it opens the step again): the region, the hour the clock
    // should show there, the place.
    let Some(region) = pick.region.clone() else {
        let mut regions: Vec<&str> = zones.iter().filter_map(|(z, _)| z.split('/').next()).collect();
        regions.dedup();
        let here = now.split('/').next().unwrap_or_default();
        view.groups.push(group(
            Some("Where are you?"),
            "zone-region",
            116.0,
            regions.iter().map(|r| choice(*r, *r == here, *r)).collect(),
        ));
        return;
    };
    view.groups.push(group(None, "zone-region", 116.0, vec![choice(region.clone(), true, "")]));
    let in_region: Vec<&(String, i32)> =
        zones.iter().filter(|(z, _)| z.split('/').next() == Some(region.as_str())).collect();
    let mut offsets: Vec<i32> = in_region.iter().map(|(_, o)| *o).collect();
    offsets.sort_unstable();
    offsets.dedup();
    let Some(offset) = pick.offset.filter(|o| offsets.contains(o)) else {
        view.groups.push(group(
            Some("What time is it there?"),
            "zone-hour",
            76.0,
            offsets.iter().map(|o| choice(clock_at(utc, *o), Some(*o) == now_offset, o.to_string())).collect(),
        ));
        return;
    };
    view.groups.push(group(None, "zone-hour", 76.0, vec![choice(clock_at(utc, offset), true, "")]));
    let mut there: Vec<&str> = in_region.iter().filter(|(_, o)| *o == offset).map(|(z, _)| z.as_str()).collect();
    there.sort_by_key(|z| place_name(z));
    // A screenful at a time; the last pill turns to the next.
    let pages = there.len().div_ceil(MAX_PLACES).max(1);
    let page = pick.page % pages;
    let mut places: Vec<Choice> = there
        .iter()
        .skip(page * MAX_PLACES)
        .take(MAX_PLACES)
        .map(|z| choice(short(&place_name(z), 14), *z == now, *z))
        .collect();
    if pages > 1 {
        places.push(choice(tr("More places"), false, ""));
    }
    view.groups.push(group(Some("Nearest place"), "zone", 130.0, places));
}

/// The file the session reads its environment from at login (uwsm): the
/// owner's language and time zone, as `export` lines between two markers.
fn session_env_path() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("uwsm")
        .join("env")
}

const ENV_BEGIN: &str = "# >>> Golem live settings (written by the control panel) >>>";
const ENV_END: &str = "# <<< Golem live settings <<<";

/// `old` with Golem's block replaced by one holding `vars` (none: no block).
pub(crate) fn session_env_text(old: &str, vars: &[(&str, String)]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut inside = false;
    for l in old.lines() {
        if l == ENV_BEGIN {
            inside = true;
        } else if l == ENV_END {
            inside = false;
        } else if !inside {
            kept.push(l);
        }
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut text = kept.join("\n");
    if !vars.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(ENV_BEGIN);
        text.push('\n');
        for (k, v) in vars {
            text.push_str(&format!("export {k}='{}'\n", v.replace('\'', "")));
        }
        text.push_str(ENV_END);
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// The session variables the owner's language and time zone stand for.
pub(crate) fn session_vars(s: &GolemSettings) -> Vec<(&'static str, String)> {
    let mut vars: Vec<(&'static str, String)> = Vec::new();
    if let Some(z) = &s.time_zone {
        vars.push(("TZ", z.clone()));
    }
    if let Some(l) = &s.language {
        vars.push(("LANG", l.clone()));
        vars.push(("LANGUAGE", l.split(['_', '.']).next().unwrap_or("en").to_owned()));
    }
    vars
}

/// Put the owner's language and time zone where the next session, and the
/// programs started from now on, find them.
pub(crate) fn assert_session_env(s: &GolemSettings) {
    let vars = session_vars(s);
    let path = session_env_path();
    // A link (home-manager's) is not ours to replace.
    if path.is_symlink() {
        debug!("controls: {} is a link; leaving the session's environment alone", path.display());
    } else {
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let new = session_env_text(&old, &vars);
        if new != old {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            crate::persist::write_text("session environment", &path, &new);
        }
    }
    // The programs the session manager starts from now on.
    for (k, v) in &vars {
        let pair = format!("{k}={v}");
        run("systemctl", &["--user", "set-environment", &pair]);
    }
}

// --------------------------------------------------------------- language

/// The languages the shell can speak: English, and every catalogue beside
/// the dictionaries (`<data-dir>/i18n/<tag>.json`).
pub(crate) fn shell_languages() -> Vec<(String, String)> {
    let mut langs = vec![("en".to_owned(), "English".to_owned())];
    for tag in crate::i18n::catalogues() {
        let name = crate::i18n::language_name(&tag);
        langs.push((tag, name));
    }
    langs
}

/// The locale a language tag stands for among the system's (`locale -a`):
/// the owner's own country first, then the language's home one.
pub(crate) fn locale_for(tag: &str, available: &[String], current: &str) -> Option<String> {
    let lang = tag.split('_').next().unwrap_or(tag);
    let utf8 = |l: &&String| l.to_lowercase().ends_with("utf8") || l.to_lowercase().ends_with("utf-8");
    let mine: Vec<&String> = available.iter().filter(utf8).filter(|l| l.split('_').next() == Some(lang)).collect();
    let country = current.split(['_', '.']).nth(1).unwrap_or_default();
    let home = match lang {
        "en" => "US",
        "es" => "ES",
        "pt" => "BR",
        "fr" => "FR",
        "de" => "DE",
        "it" => "IT",
        _ => "",
    };
    let pick = |c: &str| mine.iter().find(|l| !c.is_empty() && l.split(['_', '.']).nth(1) == Some(c));
    pick(country).or_else(|| pick(home)).or(mine.first()).map(|l| {
        // `locale -a` writes `es_ES.utf8`; the environment wants `.UTF-8`.
        let base = l.split('.').next().unwrap_or(l);
        format!("{base}.UTF-8")
    })
}

fn language_now() -> String {
    GolemSettings::load()
        .language
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_else(|| "en_US.UTF-8".into())
}

// ----------------------------------------------------------- default apps

/// The kinds of thing a default app is chosen for: (caption, the MIME type
/// that stands for the kind, every type set with it).
const KINDS: [(&str, &str, &[&str]); 6] = [
    ("Web browser", "x-scheme-handler/https", &["x-scheme-handler/http", "x-scheme-handler/https", "text/html"]),
    ("Files", "inode/directory", &["inode/directory"]),
    ("Pictures", "image/png", &["image/png", "image/jpeg", "image/webp", "image/gif"]),
    ("Documents", "application/pdf", &["application/pdf"]),
    ("Text", "text/plain", &["text/plain"]),
    ("Video", "video/mp4", &["video/mp4", "video/x-matroska", "video/webm"]),
];

/// The apps that can open a type, as (desktop id, name).
fn handlers(mime: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/share".into());
    let home = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    for dir in std::iter::once(home.as_str()).chain(dirs.split(':')) {
        let apps = std::path::Path::new(dir).join("applications");
        for e in std::fs::read_dir(&apps).into_iter().flatten().flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if !id.ends_with(".desktop") || found.iter().any(|(i, _)| *i == id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            if let Some(name) = desktop_handles(&text, mime) {
                found.push((id, name));
            }
        }
    }
    found.sort_by_key(|a| a.1.to_lowercase());
    found
}

/// The name of a desktop entry if it opens `mime` and is meant to be seen.
pub(crate) fn desktop_handles(text: &str, mime: &str) -> Option<String> {
    let mut name = None;
    let mut handles = false;
    let mut main = false;
    for l in text.lines() {
        if l.starts_with('[') {
            main = l == "[Desktop Entry]";
            continue;
        }
        if !main {
            continue;
        }
        if let Some(v) = l.strip_prefix("Name=") {
            name.get_or_insert_with(|| v.to_owned());
        } else if let Some(v) = l.strip_prefix("MimeType=") {
            handles = v.split(';').any(|m| m == mime);
        } else if l == "NoDisplay=true" || l == "Hidden=true" {
            return None;
        }
    }
    name.filter(|_| handles)
}

// -------------------------------------------------------------- wallpaper

fn wallpaper_dir() -> std::path::PathBuf {
    // (`WAVERUNNER_WALLPAPER_DIR`: another folder, for a test rig.)
    if let Some(dir) = std::env::var_os("WAVERUNNER_WALLPAPER_DIR").filter(|d| !d.is_empty()) {
        return std::path::PathBuf::from(dir);
    }
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Pictures/Wallpapers")
}

/// The program that puts a picture on the desktop (`awww`, once `swww`).
fn wallpaper_tool() -> Option<&'static str> {
    ["awww", "swww"].into_iter().find(|t| crate::launch::on_path(t))
}

/// The program that plays what moves as the desktop's picture.
const VIDEO_TOOL: &str = "mpvpaper";

/// Where it is: the one named by `WAVERUNNER_MPVPAPER` (the dev launcher
/// builds one and names it, as it does the speech engine), else the one on
/// the PATH (an installed Golem's). `None`: this computer has none.
fn video_tool() -> Option<String> {
    if let Some(named) = std::env::var("WAVERUNNER_MPVPAPER").ok().filter(|p| std::path::Path::new(p).is_file()) {
        return Some(named);
    }
    crate::launch::on_path(VIDEO_TOOL).then(|| VIDEO_TOOL.to_owned())
}

fn is_video_ext(ext: &str) -> bool {
    matches!(ext, "mp4" | "webm" | "mkv" | "mov" | "m4v")
}

/// Whether the wallpaper at `path` is a video, and whether it moves at all
/// (a video, or a GIF).
pub(crate) fn wallpaper_is_video(path: &str) -> bool {
    std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| is_video_ext(&e.to_lowercase()))
}

pub(crate) fn wallpaper_moves(path: &str) -> bool {
    wallpaper_is_video(path) || path.to_lowercase().ends_with(".gif")
}

/// How long the picture program may take over one picture. A still is
/// there in a blink, but an animated GIF is read whole and every frame of
/// it made the screen's size before the program answers: seconds, and a
/// long one many of them. (The four seconds every other program gets ended
/// it mid-work, so no GIF was ever shown — Max, 2026-10-13: "los gif están
/// ahí pero no funcionan".)
const WALLPAPER_PATIENCE: std::time::Duration = std::time::Duration::from_secs(180);

/// How the players of moving wallpapers are found: by their command line
/// (the program's own name is its wrapper's on NixOS). The bracket keeps
/// the pattern from matching the very command that carries it.
const PLAYERS: &str = "[m]pvpaper -f -o";

/// Where the player of a moving wallpaper listens (mpv's own JSON
/// commands). `None`: no runtime directory, or one with a space in its
/// path (the player's options are split at spaces).
fn player_socket() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)?;
    let socket = dir.join("waverunner-wallpaper.sock");
    (!socket.to_string_lossy().contains(' ')).then_some(socket)
}

/// Hold a moving wallpaper still, or let it go on (the overview shows the
/// desktop nine times over: Max, 2026-10-13, "quiero que el wallpaper se
/// pause en overview"). Nothing playing, or a player started before it
/// listened: nothing happens. Never waits: one short write, on its own
/// thread.
pub(crate) fn wallpaper_pause(paused: bool) {
    HELD.store(paused, std::sync::atomic::Ordering::SeqCst);
    wallpaper_tell(paused);
}

/// Why a moving wallpaper is held (`App::wallpaper_hold`).
pub(crate) const HOLD_OVERVIEW: u8 = 1;
pub(crate) const HOLD_FULLSCREEN: u8 = 2;
pub(crate) const HOLD_SCREEN: u8 = 4;

/// Whether the wallpaper is held now: a player started meanwhile is told
/// so as soon as it listens.
static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn wallpaper_tell(paused: bool) {
    let Some(socket) = player_socket() else {
        return;
    };
    std::thread::spawn(move || {
        use std::io::Write;
        if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&socket) {
            let _ = s.set_write_timeout(Some(std::time::Duration::from_millis(300)));
            let _ = writeln!(s, r#"{{"command":["set_property","pause",{paused}]}}"#);
        }
    });
}

/// The players of moving wallpapers running now, by process id.
fn players() -> Vec<String> {
    out("pgrep", &["-f", PLAYERS]).map(|found| found.split_whitespace().map(str::to_owned).collect()).unwrap_or_default()
}

/// End these players. Asked first; one that has not gone a moment later is
/// ended outright — a player whose picture is covered may not be listening,
/// and it would play on under the next one (seen in the test compositor,
/// 2026-10-13).
fn end_players(pids: &[String]) {
    if pids.is_empty() {
        return;
    }
    let alive = |pid: &str| std::path::Path::new("/proc").join(pid).exists();
    for pid in pids {
        run("kill", &[pid]);
    }
    for _ in 0..15 {
        if !pids.iter().any(|p| alive(p)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for pid in pids.iter().filter(|p| alive(p)) {
        run("kill", &["-9", pid]);
    }
}

/// A STILL of the moving wallpaper at `path` — its first frame, no wider
/// than a full-HD screen — for the picture program to hold UNDER the
/// player (see `show_wallpaper`). Made once and kept, named by the file and
/// when it last changed. `None`: it could not be made.
pub(crate) fn poster(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let changed = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.bytes().chain(meta.len().to_le_bytes()).chain(changed.to_le_bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cache")))?;
    let dir = cache.join("waverunner/posters");
    let file = dir.join(format!("{h:016x}.png"));
    let name = file.to_string_lossy().into_owned();
    if file.is_file() {
        crate::apps::still_used(&file);
        return Some(name);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let tmp = dir.join(format!("{h:016x}.tmp.png"));
    let tmp_name = tmp.to_string_lossy().into_owned();
    let made = run("ffmpegthumbnailer", &["-i", path, "-o", &tmp_name, "-s", "1920", "-t", "0"])
        && std::fs::rename(&tmp, &file).is_ok();
    if !made {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(name)
}

/// Hand a still to the picture program: `fade` with its second-long fade,
/// else at once.
fn show_still(tool: &str, path: &str, fade: bool) -> bool {
    let mut cmd = Command::new(tool);
    cmd.args(["img", path, "--transition-type"]);
    if fade {
        cmd.args(["fade", "--transition-duration", "1"]);
    } else {
        cmd.arg("none");
    }
    finish_within(cmd, false, WALLPAPER_PATIENCE).is_some()
}

/// Put the wallpaper at `path` on the desktop. A still goes through the
/// picture program (`fade`: with its second-long fade). What MOVES — a
/// video, and a GIF too — is played by `mpvpaper`, looping and silent, OVER
/// the picture program's picture: it scales on the graphics card and
/// starts at once, where the picture program makes every frame of a GIF
/// the screen's size on the processor first (9 s for a 92-frame GIF on a
/// small screen — Max, 2026-10-13: "takes for ever"). Only a GIF on a
/// computer without `mpvpaper` still goes the slow way.
///
/// NOTHING ELSE IS EVER SEEN IN BETWEEN (Max, 2026-10-13: "suele haber
/// algún wallpaper random en el medio del salto"): the picture under a
/// player was whichever still had been chosen last, and ending the old
/// player before the new one was up showed it. So the new one is up
/// BEFORE the old one goes — a new player starts over the old one, a new
/// still is laid under the old player and only then is that ended — and
/// under every player lies its own first frame (`poster`), so even a
/// player that dies leaves its own picture, not somebody else's.
/// Returns whether it was put: call it from a thread that may wait.
fn show_wallpaper(path: &str, fade: bool) -> bool {
    let old = players();
    if wallpaper_moves(path) {
        if let Some(player) = video_tool() {
            // `-f`: it forks and plays on; `*`: every screen. The graphics
            // card decodes where it can (`hwdec`): a 1080p video cost 37 %
            // of a core without it and 5 % with — a wallpaper plays all day.
            // And it listens on a socket of its own, so it can be told to
            // hold still (`wallpaper_pause`).
            let mut how = "no-audio loop-file=inf panscan=1.0 hwdec=auto-safe".to_owned();
            if let Some(socket) = player_socket() {
                let _ = std::fs::remove_file(&socket);
                how.push_str(&format!(" input-ipc-server={}", socket.display()));
            }
            if !run(&player, &["-f", "-o", &how, "*", path]) {
                return false;
            }
            // Its first frame is on the screen in a moment; then the old
            // player goes, and its own still is laid under it.
            if !old.is_empty() {
                std::thread::sleep(PLAYER_UP);
                end_players(&old);
            }
            if let (Some(tool), Some(still)) = (wallpaper_tool(), poster(path)) {
                show_still(tool, &still, false);
            }
            // (Started while the wallpaper is held: it holds too. By now it
            // has had the time to open its socket.)
            if HELD.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(300));
                wallpaper_tell(true);
            }
            return true;
        }
        if wallpaper_is_video(path) {
            return false;
        }
    }
    let Some(tool) = wallpaper_tool() else {
        return false;
    };
    // Under a player the still is laid unseen (no fade to see), and shows
    // when the player goes.
    let shown = show_still(tool, path, fade && old.is_empty());
    end_players(&old);
    shown
}

/// How long a player just started is given to put its first frame up
/// before the one it replaces is ended.
const PLAYER_UP: std::time::Duration = std::time::Duration::from_millis(450);

/// A wallpaper was picked: it is put up on a thread of its own, so a GIF's
/// seconds of preparation hold up neither the settings nor a later pick,
/// and remembered once it is up. A newer pick overtakes an older one still
/// being prepared (`WALLPAPER_PICK`: only the newest is remembered).
fn pick_wallpaper(path: String) {
    static WALLPAPER_PICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mine = WALLPAPER_PICK.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    *WALLPAPER_WANT.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    let spawned = std::thread::Builder::new().name("wallpaper".into()).spawn(move || {
        // One at a time; a pick overtaken while it waited is not put up.
        let _one = WALLPAPER_ONE.lock().unwrap_or_else(|e| e.into_inner());
        let newest = || WALLPAPER_PICK.load(std::sync::atomic::Ordering::SeqCst) == mine;
        if !newest() {
            return;
        }
        let shown = show_wallpaper(&path, true);
        if !shown {
            warn!("controls: the wallpaper {path:?} could not be put up");
        }
        if !newest() {
            return;
        }
        // Up (or not): the worker saves it and reads the setting again, so
        // the box shows what is true now.
        let key = if shown { "wallpaper-saved" } else { "wallpaper-failed" };
        if let Some(door) = TO_WORKER.get() {
            let cmd = ControlCommand::Set { label: "Wallpaper", key, value: path, ui: UiFacts::default() };
            let _ = door.lock().unwrap_or_else(|e| e.into_inner()).send(cmd);
        }
    });
    if let Err(e) = spawned {
        warn!("controls: no thread for the wallpaper: {e}");
    }
}

/// The most pictures shown: three pages of the wall (`panel.rs`
/// `TILES_PER_PAGE`).
const MAX_PICTURES: usize = 3 * crate::panel::TILES_PER_PAGE;

/// The wallpapers the box will show, as paths (one look at the folder:
/// quick enough for whoever asks).
pub(crate) fn wallpaper_files() -> Vec<String> {
    if wallpaper_tool().is_none() {
        return Vec::new();
    }
    pictures().iter().take(MAX_PICTURES).map(|p| p.to_string_lossy().into_owned()).collect()
}

fn pictures() -> Vec<std::path::PathBuf> {
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(wallpaper_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            // Stills, and what moves: an animated GIF is the picture
            // program's own to play; a video is played by `mpvpaper`, so
            // videos are listed only where that is installed.
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                let e = e.to_lowercase();
                matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif")
                    || (is_video_ext(&e) && video_tool().is_some())
            })
        })
        .collect();
    found.sort();
    found
}

// ------------------------------------------------------------------- read

/// Read the setting `label` as it is now.
pub(crate) fn read(label: &'static str, ui: &UiFacts, zone: &mut ZonePick) -> ControlView {
    let mut view = ControlView { label, ..Default::default() };
    match label {
        "Wi-Fi" => match wifi_state() {
            Some((on, name)) => {
                view.subtitle = match (on, name) {
                    (false, _) => tr("Wi-Fi is off").to_owned(),
                    (true, Some(n)) => tr("Connected to {name}").replace("{name}", &n),
                    (true, None) => tr("Not connected").to_owned(),
                };
                view.groups.push(on_off(None, "wifi", on));
                view.groups.push(more("net"));
            }
            None => view.subtitle = tr("This computer has no Wi-Fi.").to_owned(),
        },
        "Bluetooth" => match bt_powered() {
            Some(on) => {
                view.subtitle = tr(if on { "Bluetooth is on" } else { "Bluetooth is off" }).to_owned();
                view.groups.push(on_off(None, "bluetooth", on));
                if ui.has_bt {
                    view.groups.push(more("bt"));
                }
            }
            None => view.subtitle = tr("This computer has no Bluetooth.").to_owned(),
        },
        "Sound output" => read_devices(&mut view, "Audio/Sink", "sink", "Nothing to play sound through."),
        "Volume" => match level_group("Volume", "volume", "@DEFAULT_AUDIO_SINK@") {
            Some((levels, mute, level, muted)) => {
                view.subtitle = if muted { tr("Muted").to_owned() } else { format!("{:.0}%", level * 100.0) };
                view.groups.push(Group { caption: None, ..levels });
                view.groups.push(mute);
            }
            None => view.subtitle = tr("Nothing to play sound through.").to_owned(),
        },
        "Microphone" => {
            read_devices(&mut view, "Audio/Source", "source", "No microphone.");
            if let Some((levels, mute, _, _)) = level_group("Level", "mic-level", "@DEFAULT_AUDIO_SOURCE@") {
                view.groups.push(levels);
                view.groups.push(mute);
            }
        }
        // ONE PAGE for how everything looks (Max, 2026-10-15): the look, where
        // the colours go, then the palettes — the Dynamic one (made from the
        // wallpaper; the shell keeps reading the screen place by place) first.
        "Colours" if EDITOR.lock().is_ok_and(|g| g.is_some()) => {
            let e = EDITOR.lock().ok().and_then(|g| g.clone()).unwrap_or(Editor { soul: [[0; 3]; 6], cur: 0, hue: 0.0 });
            view.subtitle = tr("Pick a place, then its colour. Each goes exactly there.").to_owned();
            let roles = crate::palette_gen::ROLES
                .iter()
                .enumerate()
                .map(|(i, r)| Choice { swatch: vec![e.soul[i]], ..choice(tr(r), i == e.cur, i.to_string()) })
                .collect();
            view.groups.push(group(None, "colors-edit-role", 104.0, roles));
            let mut field = Vec::new();
            for row in 0..FIELD_ROWS {
                for col in 0..FIELD_COLS {
                    let c = field_colour(col, row, e.hue);
                    let v = format!("#{}", crate::palette::hex(c));
                    field.push(Choice { swatch: vec![c], ..choice(String::new(), c == e.soul[e.cur], v) });
                }
            }
            view.groups.push(Group { cells: FIELD_COLS, ..group(None, "colors-edit-colour", 0.0, field) });
            let hues = (0..HUES)
                .map(|k| {
                    let h = 360.0 * k as f32 / HUES as f32;
                    let c = crate::palette::tint::hue(k as f32 / HUES as f32);
                    let d = (h - e.hue).abs();
                    let near = d.min(360.0 - d) <= 180.0 / HUES as f32;
                    Choice { swatch: vec![c], ..choice(String::new(), near, format!("{h:.0}")) }
                })
                .collect();
            view.groups.push(Group { cells: HUES, ..group(None, "colors-edit-hue", 0.0, hues) });
            view.groups.push(group(None, "colors-edit", 120.0, vec![choice(tr("Save"), false, "save"), choice(tr("Cancel"), false, "cancel")]));
        }
        "Colours" if APPS_LIST.load(std::sync::atomic::Ordering::Relaxed) => {
            let picked = crate::palette::choice();
            view.subtitle = tr("Lit: the app wears your colours. Off: its own. Most used first.").to_owned();
            view.groups.push(group(None, "colors-apps-list", 120.0, vec![choice(tr("‹  Back"), false, "back")]));
            let apps = ui
                .apps
                .iter()
                .take(APPS_SHOWN)
                .map(|(id, name)| choice(name.clone(), !picked.skip.contains(id), id.clone()))
                .collect();
            view.groups.push(group(None, "colors-skip", 150.0, apps));
        }
        "Colours" => {
            let dark = dark_now();
            let picked = crate::palette::choice();
            view.subtitle = tr("How everything looks. Some apps change the next time they open.").to_owned();
            view.groups.push(look_pair(None, "dark", dark, "on", "off"));
            // The three sliders (Max, 2026-10-16): 0 = Off (the makers' own
            // colours), the middle = the palette as made, the end = full
            // colour; each with a [Dark][Light] of its own that follows the
            // system's until it is clicked.
            let apps = picked.apps_level().unwrap_or(MADE);
            let web = picked.web_level().unwrap_or(MADE);
            for (name, key, look_key, level, own) in [
                ("Apps", "colors-apps", "colors-apps-look", apps, picked.apps_dark),
                ("Web", "colors-web", "colors-web-look", web, picked.web_dark),
                ("Icons", "colors-icons", "colors-icons-look", picked.icons_tint, picked.icons_dark),
            ] {
                let caption = format!("{} · {}", tr(name), level_text(level));
                view.groups.push(Group { caption: Some(caption), ..slider(key, level) });
                let mut pair = look_pair(None, look_key, own.unwrap_or(dark), "dark", "light");
                if key == "colors-apps" {
                    // The small round "+": the apps, one switch each.
                    pair.choices.push(choice("+", false, "list"));
                }
                view.groups.push(Group { beside: true, ..pair });
            }
            let now = picked.palette_id().to_owned();
            let ground = |p: &crate::palette::Palette| if dark { p.dark[0] } else { p.light[0] };
            let mut choices = vec![Choice { swatch: Vec::new(), ..choice(tr("Dynamic"), now == crate::palette::DYNAMIC_ID, crate::palette::DYNAMIC_ID) }];
            choices.extend(crate::palette::all().into_iter().map(|p| {
                let mut swatch = vec![ground(&p)];
                swatch.extend(if p.soul.is_empty() { p.dark[8..16].to_vec() } else { p.soul.clone() });
                Choice { swatch, ..choice(p.name.clone(), p.id == now, p.id.clone()) }
            }));
            // The "+" tile at the end: make one's own.
            choices.push(choice("+", false, "new"));
            view.groups.push(Group { tiles: true, ..group(None, "colors-palette", 110.0, choices) });
        }
        "Brightness" => match brightness() {
            Some(now) => {
                view.subtitle = format!("{:.0}%", now * 100.0);
                let lit = nearest(&BRIGHT, now);
                let levels = BRIGHT
                    .iter()
                    .enumerate()
                    .map(|(i, l)| choice(format!("{:.0}%", l * 100.0), i == lit, format!("{:.0}", l * 100.0)))
                    .collect();
                view.groups.push(group(None, "brightness", 76.0, levels));
                view.groups.push(more("gear"));
            }
            None => view.subtitle = tr("This screen sets its own brightness.").to_owned(),
        },
        "Night light" => {
            let k = if ui.night_k == 0 { crate::screen::NEUTRAL_K } else { ui.night_k };
            let stops: [(u32, &str); 4] =
                [(crate::screen::NEUTRAL_K, "Off"), (5000, "Warm"), (4000, "Warmer"), (3000, "Warmest")];
            let lit = stops.iter().enumerate().min_by_key(|(_, (s, _))| s.abs_diff(k)).map_or(0, |(i, _)| i);
            view.subtitle = tr("A warmer screen is easier on the eyes at night.").to_owned();
            view.groups.push(group(
                None,
                "night",
                110.0,
                stops.iter().enumerate().map(|(i, (s, name))| choice(tr(name), i == lit, s.to_string())).collect(),
            ));
        }
        "Notifications" => {
            view.subtitle = tr(if ui.dnd {
                "Do not disturb is on: nothing pops up."
            } else {
                "Notifications show as they arrive."
            })
            .to_owned();
            view.groups.push(on_off(Some("Do not disturb"), "dnd", ui.dnd));
        }
        "Keyboard layout" => {
            let (layout, variant) = keyboard_now();
            let known = LAYOUTS.iter().find(|(_, l, v)| *l == layout && *v == variant);
            view.subtitle = known.map_or_else(|| format!("{layout} {variant}").trim().to_owned(), |k| k.0.to_owned());
            let mut choices: Vec<Choice> = LAYOUTS
                .iter()
                .map(|(name, l, v)| choice(*name, *l == layout && *v == variant, format!("{l}:{v}")))
                .collect();
            if known.is_none() {
                choices.insert(0, choice(view.subtitle.clone(), true, format!("{layout}:{variant}")));
            }
            view.groups.push(group(None, "keyboard", 170.0, choices));
        }
        "Power mode" => match out("powerprofilesctl", &["list"]) {
            Some(list) => {
                let names = crate::sys::parse_profiles(&list);
                let now = out("powerprofilesctl", &["get"]).unwrap_or_default();
                let now = now.trim();
                let label = |n: &str| match n {
                    "power-saver" => tr("Saver"),
                    "balanced" => tr("Balanced"),
                    _ => tr("Performance"),
                };
                view.subtitle = label(now).to_owned();
                view.groups.push(group(
                    None,
                    "profile",
                    130.0,
                    names.iter().map(|n| choice(label(n), n == now, n.clone())).collect(),
                ));
                if ui.has_battery {
                    view.groups.push(more("bat"));
                }
            }
            None => view.subtitle = tr("This computer has one power mode.").to_owned(),
        },
        "Touchpad speed" => {
            let now = GolemSettings::load().pointer_speed.unwrap_or(0.0);
            let stops: Vec<f64> = SPEEDS.iter().map(|s| s.0).collect();
            let lit = nearest(&stops, now);
            view.subtitle = tr("How far the pointer moves. The mouse follows it too.").to_owned();
            view.groups.push(group(
                None,
                "speed",
                110.0,
                SPEEDS.iter().enumerate().map(|(i, (v, name))| choice(tr(name), i == lit, v.to_string())).collect(),
            ));
        }
        "Natural scrolling" => {
            let on = GolemSettings::load().natural_scroll.unwrap_or(true);
            view.subtitle = tr(if on {
                "The page follows your fingers."
            } else {
                "The page moves against your fingers."
            })
            .to_owned();
            view.groups.push(on_off(None, "natural", on));
        }
        "Updates" => {
            view.subtitle = if ui.updates.is_empty() { tr("Up to date").to_owned() } else { ui.updates.clone() };
            view.groups.push(group(None, "update", 150.0, vec![choice(tr("Check now"), false, "check")]));
            view.groups.push(more("gear"));
        }
        // How the windows sit and stick. Empty for now: the window controls
        // come here as they are made (Max, 2026-10-14).
        "Tiles" => {
            view.subtitle = tr("How windows sit, stick and line up.").to_owned();
        }
        "About Golem" => {
            view.subtitle = os_name();
            view.groups.extend(machine_lines().into_iter().map(line));
            view.groups.push(more("gear"));
        }
        "Battery" => match battery_line() {
            Some(text) => {
                view.subtitle = text;
                if let Some(limit) = charge_limit() {
                    view.groups.push(group(
                        Some("Stop charging at"),
                        "charge-limit",
                        96.0,
                        vec![choice("80%", limit, "on"), choice("100%", !limit, "off")],
                    ));
                }
                if ui.has_battery {
                    view.groups.push(more("bat"));
                }
            }
            None => view.subtitle = tr("No battery in this computer").to_owned(),
        },
        "Privacy" => {
            let recording = parse_recording(&sound_dump());
            let camera = camera_in_use();
            view.subtitle = match (recording.is_empty(), camera) {
                (true, false) => tr("Nothing is using the microphone or the camera.").to_owned(),
                (true, true) => tr("The camera is in use.").to_owned(),
                (false, cam) => {
                    let who = recording.join(", ");
                    let text = tr("The microphone is in use by {apps}.").replace("{apps}", &who);
                    if cam { format!("{text} {}", tr("The camera is in use.")) } else { text }
                }
            };
            if let Some((_, muted)) =
                out("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"]).and_then(|t| parse_volume(&t))
            {
                view.groups.push(group(
                    Some("Microphone"),
                    "mic-mute",
                    110.0,
                    vec![choice(tr("On"), !muted, "off"), choice(tr("Muted"), muted, "on")],
                ));
            }
        }
        "Time zone" => read_zone(&mut view, zone),
        "Language" => {
            let now = language_now();
            let now_lang = now.split(['_', '.']).next().unwrap_or("en").to_owned();
            let langs = shell_languages();
            view.subtitle = tr("The language Golem speaks. Apps follow the next time they open.").to_owned();
            view.groups.push(group(
                None,
                "language",
                150.0,
                langs.iter().map(|(tag, name)| choice(name.clone(), *tag == now_lang, tag.clone())).collect(),
            ));
        }
        "Default apps" => {
            view.subtitle = tr("What opens each kind of thing.").to_owned();
            for (caption, mime, _) in KINDS {
                let apps = handlers(mime);
                if apps.is_empty() {
                    continue;
                }
                let now = out("xdg-mime", &["query", "default", mime]).unwrap_or_default();
                let now = now.trim();
                view.groups.push(group(
                    Some(caption),
                    kind_key(mime),
                    190.0,
                    apps.iter().take(6).map(|(id, name)| choice(short(name, 20), id == now, id.clone())).collect(),
                ));
            }
        }
        "Wallpaper" => {
            let found = pictures();
            let want = WALLPAPER_WANT.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let now = want.or_else(|| GolemSettings::load().wallpaper);
            view.subtitle = if found.is_empty() {
                tr("Put pictures in Pictures/Wallpapers to choose one here.").to_owned()
            } else if wallpaper_tool().is_none() {
                tr("This computer cannot change its wallpaper yet.").to_owned()
            } else {
                tr("The pictures in Pictures/Wallpapers.").to_owned()
            };
            if !found.is_empty() && wallpaper_tool().is_some() {
                let choices = found
                    .iter()
                    .take(MAX_PICTURES)
                    .map(|p| {
                        let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        let path = p.to_string_lossy().into_owned();
                        choice(short(&name.replace(['-', '_'], " "), 18), now.as_deref() == Some(path.as_str()), path)
                    })
                    .collect();
                view.groups.push(Group { tiles: true, ..group(None, "wallpaper", 170.0, choices) });
            }
        }
        _ => {}
    }
    view
}

/// The group key of a kind of default app.
fn kind_key(mime: &str) -> &'static str {
    match mime {
        "x-scheme-handler/https" => "default:web",
        "inode/directory" => "default:files",
        "image/png" => "default:pictures",
        "application/pdf" => "default:documents",
        "text/plain" => "default:text",
        _ => "default:video",
    }
}

/// The battery's charge limit, where the kernel offers one: whether it is
/// held at 80%.
fn charge_limit() -> Option<bool> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    dir.flatten().find_map(|d| {
        let v = std::fs::read_to_string(d.path().join("charge_control_end_threshold")).ok()?;
        Some(v.trim().parse::<u32>().ok()? < 100)
    })
}

// ------------------------------------------------------------------ apply

/// What a pick changed beyond the setting itself.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    Done,
    /// The shell's own language: `App` loads its strings again.
    Language,
    /// The time zone: `App` moves its own clocks.
    Zone(String),
}

/// Take the chosen time zone as this process's own, so the shell's clocks
/// show it. At start it runs before any thread exists. Later (a pick in the
/// panel) it runs on the loop: the environment is only ever read by the C
/// library's clock calls, which take its lock, and written here.
pub(crate) fn adopt_zone(zone: Option<&str>) {
    let Some(zone) = zone.filter(|z| z.contains('/') && z.chars().all(|c| c.is_ascii_alphanumeric() || "/_-+".contains(c)))
    else {
        return;
    };
    std::env::set_var("TZ", zone);
    extern "C" {
        fn tzset();
    }
    // SAFETY: `tzset` reads `TZ` and sets the C library's zone, under its lock.
    unsafe { tzset() };
}

/// Carry out a pick of `value` in the group `key`.
pub(crate) fn apply(key: &str, value: &str, zone: &mut ZonePick) -> Applied {
    let on = value == "on";
    match key {
        "wifi" => {
            run("nmcli", &["radio", "wifi", value]);
        }
        "bluetooth" => {
            run("bluetoothctl", &["power", value]);
        }
        "sink" | "source" => {
            run("wpctl", &["set-default", value]);
        }
        "volume" => {
            run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "0"]);
            run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", value]);
        }
        "mute" => {
            run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", if on { "1" } else { "0" }]);
        }
        "mic-level" => {
            run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SOURCE@", "0"]);
            run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SOURCE@", value]);
        }
        "mic-mute" => {
            run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SOURCE@", if on { "1" } else { "0" }]);
        }
        "dark" => {
            GolemSettings::change(|s| {
                s.dark = Some(on);
                s.store();
            });
            assert_dark(on);
        }
        "brightness" => {
            run("golem-brightness", &["set", &format!("{value}%")]);
        }
        "keyboard" => {
            let (layout, variant) = value.split_once(':').unwrap_or((value, ""));
            if valid_xkb(layout) && valid_xkb(variant) && !layout.is_empty() {
                GolemSettings::change(|s| s.set_keyboard(layout, variant));
            } else {
                warn!("controls: not a keyboard layout: {value:?}");
            }
        }
        "profile" => {
            run("powerprofilesctl", &["set", value]);
        }
        "speed" => {
            if let Ok(v) = value.parse::<f64>() {
                GolemSettings::change(|s| s.set_pointer_speed(v.clamp(-1.0, 1.0)));
            }
        }
        "natural" => {
            GolemSettings::change(|s| s.set_natural_scroll(on));
        }
        "update" => {
            run("systemctl", &["start", "--no-block", "golem-autoupdate.service"]);
        }
        "charge-limit" => {
            set_charge_limit(on);
        }
        // An empty value is a click on a folded step: open it again.
        "zone-region" => {
            zone.region = Some(value.to_owned()).filter(|v| !v.is_empty());
            zone.offset = None;
            zone.page = 0;
        }
        "zone-hour" => {
            zone.offset = value.parse().ok();
            zone.page = 0;
        }
        // "More places".
        "zone" if value.is_empty() => zone.page += 1,
        "zone" => {
            let known = zone.zones.as_ref().is_some_and(|z| z.iter().any(|(n, _)| n == value));
            if known {
                GolemSettings::change(|s| {
                    s.time_zone = Some(value.to_owned());
                    s.store();
                    assert_session_env(s);
                });
                *zone = ZonePick { zones: zone.zones.take(), ..Default::default() };
                return Applied::Zone(value.to_owned());
            } else {
                warn!("controls: not a time zone: {value:?}");
            }
        }
        "language" => {
            let available: Vec<String> =
                out("locale", &["-a"]).unwrap_or_default().lines().map(str::to_owned).collect();
            let current = language_now();
            let locale = if value == "en" && available.is_empty() {
                Some("en_US.UTF-8".to_owned())
            } else {
                locale_for(value, &available, &current)
            };
            match locale {
                Some(locale) => {
                    GolemSettings::change(|s| {
                        s.language = Some(locale);
                        s.store();
                        assert_session_env(s);
                    });
                    return Applied::Language;
                }
                None => warn!("controls: this system has no locale for {value:?}"),
            }
        }
        "wallpaper" => {
            let path = std::path::Path::new(value);
            if path.starts_with(wallpaper_dir()) && path.is_file() {
                pick_wallpaper(value.to_owned());
            }
        }
        // The wallpaper's own thread reporting back (`pick_wallpaper`).
        "wallpaper-saved" | "wallpaper-failed" => {
            let mut want = WALLPAPER_WANT.lock().unwrap_or_else(|e| e.into_inner());
            if want.as_deref() == Some(value) {
                *want = None;
            }
            drop(want);
            if key == "wallpaper-saved" {
                GolemSettings::change(|s| {
                    s.wallpaper = Some(value.to_owned());
                    s.store();
                });
            }
        }
        // The wallpapers' box was left: what the picture program kept of the
        // GIFs it prepared (hundreds of megabytes each) goes.
        "wallpaper-leave" => {
            if let Some(tool) = wallpaper_tool() {
                run(tool, &["clear-cache"]);
            }
        }
        k if k.starts_with("default:") => {
            if let Some((_, _, types)) = KINDS.iter().find(|(_, mime, _)| kind_key(mime) == k) {
                if value.ends_with(".desktop") && !value.contains('/') {
                    let mut args = vec!["default", value];
                    args.extend(types.iter().copied());
                    run("xdg-mime", &args);
                }
            }
        }
        _ => debug!("controls: nothing sets {key:?}"),
    }
    Applied::Done
}

/// Hold the battery at 80%, or let it fill (the gear box's own switch: the
/// system hands the file to the owner, see Golem's `desktop/care.nix`).
fn set_charge_limit(on: bool) {
    let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") else {
        return;
    };
    for d in dir.flatten() {
        let f = d.path().join("charge_control_end_threshold");
        if f.exists() {
            if let Err(e) = std::fs::write(&f, if on { "80" } else { "100" }) {
                warn!("controls: charge limit: {e}");
            }
        }
    }
}

/// Put the saved choices that live outside the compositor back, at start:
/// the apps' look (the home layer writes its own default at every rebuild)
/// and the session's language and zone.
pub(crate) fn reassert() {
    let s = GolemSettings::load();
    // The compositor's own settings: on it now, and in the file it reads at
    // its start (a settings.json the installer seeded has no such file yet).
    s.reassert_input();
    if s.dark.is_none() && s.language.is_none() && s.time_zone.is_none() && s.wallpaper.is_none() {
        return;
    }
    let spawned = std::thread::Builder::new().name("controls-reassert".into()).spawn(move || {
        if let Some(dark) = s.dark {
            assert_dark(dark);
        }
        if s.language.is_some() || s.time_zone.is_some() {
            assert_session_env(&s);
        }
        // The session starts with no picture at all (the compositor's bare
        // grey): the owner's goes up as soon as the picture program answers
        // — it is started beside us, so a few tries may pass first.
        if let Some(picture) = &s.wallpaper {
            if let Some(tool) = wallpaper_tool() {
                for _ in 0..40 {
                    if run(tool, &["query"]) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            if std::path::Path::new(picture).is_file() {
                let _one = WALLPAPER_ONE.lock().unwrap_or_else(|e| e.into_inner());
                // (A pick made while this waited has the say.)
                if WALLPAPER_WANT.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                    show_wallpaper(picture, false);
                }
            }
        }
    });
    if let Err(e) = spawned {
        warn!("controls: could not put the saved settings back: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_devices_and_the_default_come_out_of_a_dump() {
        let dump = serde_json::json!([
            { "id": 40, "type": "PipeWire:Interface:Node", "info": { "props": {
                "media.class": "Audio/Sink", "node.name": "alsa.speakers", "node.description": "Speakers" } } },
            { "id": 52, "type": "PipeWire:Interface:Node", "info": { "props": {
                "media.class": "Audio/Sink", "node.name": "bluez.headset", "node.description": "Headset" } } },
            { "id": 60, "type": "PipeWire:Interface:Node", "info": { "props": {
                "media.class": "Audio/Source", "node.name": "alsa.mic", "node.description": "Built-in mic" } } },
            { "id": 33, "type": "PipeWire:Interface:Metadata", "props": { "metadata.name": "default" },
              "metadata": [
                { "key": "default.audio.sink", "value": { "name": "bluez.headset" } },
                { "key": "default.audio.source", "value": { "name": "alsa.mic" } } ] }
        ]);
        let (sinks, default) = parse_sound(&dump, "Audio/Sink");
        assert_eq!(sinks.iter().map(|n| n.description.as_str()).collect::<Vec<_>>(), ["Headset", "Speakers"]);
        assert_eq!(default.as_deref(), Some("bluez.headset"));
        let (sources, default) = parse_sound(&dump, "Audio/Source");
        assert_eq!((sources.len(), default.as_deref()), (1, Some("alsa.mic")));
        assert_eq!(parse_sound(&serde_json::Value::Null, "Audio/Sink"), (Vec::new(), None));
    }

    #[test]
    fn a_volume_line_reads_as_level_and_mute() {
        assert_eq!(parse_volume("Volume: 0.45\n"), Some((0.45, false)));
        assert_eq!(parse_volume("Volume: 1.00 [MUTED]"), Some((1.0, true)));
        assert_eq!(parse_volume("nothing"), None);
    }

    #[test]
    fn the_nearest_stop_is_the_lit_one() {
        assert_eq!(nearest(&LEVELS, 0.45), 2);
        assert_eq!(nearest(&LEVELS, 0.0), 0);
        assert_eq!(nearest(&LEVELS, 1.4), 4);
    }

    #[test]
    fn only_xkb_names_reach_the_compositor() {
        assert!(valid_xkb("latam") && valid_xkb("altgr-intl") && valid_xkb(""));
        assert!(!valid_xkb("us\" })") && !valid_xkb("a b"));
    }

    #[test]
    fn only_someone_else_recording_counts() {
        let dump = serde_json::json!([
            { "info": { "state": "running", "props": { "media.class": "Stream/Input/Audio", "application.name": "Zoom" } } },
            { "info": { "state": "running", "props": { "media.class": "Stream/Input/Audio", "application.name": "cava" } } },
            { "info": { "state": "idle", "props": { "media.class": "Stream/Input/Audio", "application.name": "OBS" } } },
            { "info": { "state": "running", "props": { "media.class": "Stream/Output/Audio", "application.name": "Music" } } }
        ]);
        assert_eq!(parse_recording(&dump), ["Zoom"]);
    }

    #[test]
    fn a_zone_starts_at_the_regions_and_a_folded_step_opens_again() {
        let mut zone = ZonePick {
            zones: Some(vec![("America/La_Paz".into(), -240), ("America/Lima".into(), -300), ("Europe/Madrid".into(), 120)]),
            ..Default::default()
        };
        let mut view = ControlView::default();
        read_zone(&mut view, &mut zone);
        assert_eq!(view.groups.iter().map(|g| g.key).collect::<Vec<_>>(), ["zone-region"]);
        assert_eq!(view.groups[0].choices.len(), 2, "America and Europe");
        apply("zone-region", "America", &mut zone);
        let mut view = ControlView::default();
        read_zone(&mut view, &mut zone);
        assert_eq!(view.groups.iter().map(|g| g.key).collect::<Vec<_>>(), ["zone-region", "zone-hour"]);
        assert_eq!(view.groups[1].choices.len(), 2, "two hours in America");
        // A click on the folded region: back to the regions.
        apply("zone-region", "", &mut zone);
        let mut view = ControlView::default();
        read_zone(&mut view, &mut zone);
        assert_eq!(view.groups.len(), 1);
    }

    #[test]
    fn many_places_come_a_screenful_at_a_time() {
        let zones: Vec<(String, i32)> = (0..20).map(|i| (format!("Asia/Place{i:02}"), 60)).collect();
        let mut zone = ZonePick { zones: Some(zones), region: Some("Asia".into()), offset: Some(60), ..Default::default() };
        let mut view = ControlView::default();
        read_zone(&mut view, &mut zone);
        let places = &view.groups[2].choices;
        assert_eq!(places.len(), MAX_PLACES + 1);
        assert_eq!((places[0].text.as_str(), places[MAX_PLACES].text.as_str()), ("Place00", "More places"));
        apply("zone", "", &mut zone);
        let mut view = ControlView::default();
        read_zone(&mut view, &mut zone);
        assert_eq!(view.groups[2].choices[0].text, "Place15");
        assert_eq!(view.groups[2].choices.len(), 5 + 1);
    }

    #[test]
    fn the_zone_list_is_the_third_column() {
        let tab = "# comment\nBO\t-1630-06809\tAmerica/La_Paz\nES,MA\t+4024-00341\tEurope/Madrid\tSpain (mainland)\n";
        assert_eq!(zone_tab_names(tab), ["America/La_Paz", "Europe/Madrid"]);
    }

    #[test]
    fn the_backlight_reads_from_the_helpers_line() {
        assert_eq!(parse_brightness("intel_backlight,backlight,4648,50%,19200\n"), Some(0.5));
        assert_eq!(parse_brightness("nothing"), None);
    }

    #[test]
    fn offsets_and_clocks() {
        assert_eq!(parse_offset("-0400"), Some(-240));
        assert_eq!(parse_offset("+0530\n"), Some(330));
        assert_eq!(parse_offset("0530"), None);
        assert_eq!(clock_at(60, -240), "21:00");
        assert_eq!(clock_at(23 * 60 + 50, 330), "05:20");
        assert_eq!(place_name("America/Argentina/Buenos_Aires"), "Buenos Aires");
    }

    #[test]
    fn the_session_file_keeps_what_is_not_ours() {
        let vars = [("TZ", "America/La_Paz".to_owned())];
        let first = session_env_text("export EDITOR=nvim\n", &vars);
        assert!(first.starts_with("export EDITOR=nvim\n\n# >>> Golem"));
        assert!(first.contains("export TZ='America/La_Paz'\n"));
        // Written again, it is the same file; another zone replaces the line.
        assert_eq!(session_env_text(&first, &vars), first);
        let second = session_env_text(&first, &[("TZ", "Europe/Madrid".to_owned())]);
        assert!(second.contains("Europe/Madrid") && !second.contains("La_Paz"));
        // Nothing chosen: the block goes, the rest stays.
        assert_eq!(session_env_text(&second, &[]), "export EDITOR=nvim\n");
        assert_eq!(session_env_text("", &[]), "");
    }

    #[test]
    fn a_language_finds_its_locale() {
        let have: Vec<String> =
            ["C", "en_US.utf8", "es_ES.utf8", "es_BO.utf8", "pt_BR.utf8"].iter().map(|s| (*s).to_owned()).collect();
        // The owner's own country first.
        assert_eq!(locale_for("es", &have, "en_BO.UTF-8").as_deref(), Some("es_BO.UTF-8"));
        assert_eq!(locale_for("es", &have, "en_US.UTF-8").as_deref(), Some("es_ES.UTF-8"));
        assert_eq!(locale_for("pt", &have, "es_BO.UTF-8").as_deref(), Some("pt_BR.UTF-8"));
        assert_eq!(locale_for("de", &have, "en_US.UTF-8"), None);
    }

    #[test]
    fn a_desktop_entry_is_a_handler_only_for_its_types() {
        let entry = "[Desktop Entry]\nName=Loupe\nMimeType=image/png;image/jpeg;\n[Desktop Action x]\nName=Other\n";
        assert_eq!(desktop_handles(entry, "image/png").as_deref(), Some("Loupe"));
        assert_eq!(desktop_handles(entry, "application/pdf"), None);
        assert_eq!(desktop_handles("[Desktop Entry]\nName=X\nNoDisplay=true\nMimeType=image/png;\n", "image/png"), None);
    }

    #[test]
    fn every_setting_reads_without_a_machine_behind_it() {
        // No assertion on the content (it is this machine's): each read must
        // come back, with its own label.
        let mut zone = ZonePick { zones: Some(vec![("Europe/Madrid".into(), 120), ("America/La_Paz".into(), -240)]), ..Default::default() };
        for label in ["Night light", "Notifications", "Time zone", "About Golem"] {
            let view = read(label, &UiFacts::default(), &mut zone);
            assert_eq!(view.label, label);
        }
    }

    #[test]
    fn choosing_a_zone_goes_region_hour_place() {
        let mut zone = ZonePick {
            zones: Some(vec![
                ("America/La_Paz".into(), -240),
                ("America/Caracas".into(), -240),
                ("America/Lima".into(), -300),
                ("Europe/Madrid".into(), 120),
            ]),
            ..Default::default()
        };
        apply("zone-region", "America", &mut zone);
        apply("zone-hour", "-240", &mut zone);
        let mut view = ControlView::default();
        read_zone(&mut view, &mut zone);
        let keys: Vec<&str> = view.groups.iter().map(|g| g.key).collect();
        assert_eq!(keys, ["zone-region", "zone-hour", "zone"]);
        assert_eq!(view.groups[0].choices.len(), 1, "a finished step is its one pill");
        let places: Vec<&str> = view.groups[2].choices.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(places, ["Caracas", "La Paz"]);
    }
}

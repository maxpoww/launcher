//! THE QUICK SETTINGS — the gear box's first page holds what is needed at hand
//! (Max, 2026-11-05: "esta caja es nuestro quick settings… ahí tiene que estar
//! todo lo que el usuario necesite a mano"; the kit's sheet
//! https://claude.ai/artifact/QSig7uAfBuGvzASySGNNEg, board "Quick settings"):
//! SOUND AND SCREEN — the volume (its symbol mutes) with the output it plays
//! through (›: a page to choose another), the brightness with Auto, the
//! microphone —, then MODES — night light, dark mode, do not disturb (the
//! machine page adds airplane mode, keep awake, floating windows, the power
//! mode).
//!
//! Each is done the way the control panel does it (`controls::apply`, run on
//! a thread of this page's own that keeps only the newest of each key, so a
//! level dragged sends what the hand says now, not every step it passed);
//! night light and do not disturb are this thread's (`set_screen_temperature`,
//! `toggle_notif_mute`); dark mode is the colours model's (`colors_dark_all`).
//! What the hand set is shown at once and held `HOLD` over the readings (the
//! brain's sound inventory, the machine page's `mic_muted` / `bright`), which
//! catch up a moment later.

use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::gear::{Hit, Item, Tone, Trail, View};
use crate::i18n::tr;
use crate::App;

const G_VOLUME: &str = "\u{f057e}";
const G_VOLUME_LOW: &str = "\u{f0580}";
const G_VOLUME_OFF: &str = "\u{f0581}";
const G_BRIGHT: &str = "\u{f00df}";
const G_MIC: &str = "\u{f036c}";
const G_MIC_OFF: &str = "\u{f036d}";
const G_NIGHT: &str = "\u{f0594}";
const G_DARK: &str = "\u{f050e}";
const G_DND: &str = "\u{f009b}";
const G_SPEAKER: &str = "\u{f04c3}";
const G_HEADPHONES: &str = "\u{f02cb}";
const G_SCREEN: &str = "\u{f0379}";
const G_CHECK: &str = "\u{f00c}";

/// How long what the hand set is shown over the readings.
const HOLD: Duration = Duration::from_secs(3);
/// A notch of the wheel over a level: this much of it.
const NOTCH: f32 = 0.05;
/// The wheel's units in a notch (the gear's `SCROLL_SPEED` figure's base).
const NOTCH_UNITS: f32 = 10.0;
/// Night light turned on with no warmth chosen before: the control panel's
/// "Warmer".
const NIGHT_K: u32 = 4000;

/// What a press on the quick settings lands on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum QuickHit {
    Volume,
    Mute,
    /// The output under the volume: its page.
    Output,
    /// An output on that page (wpctl's id).
    OutputPick(u32),
    /// THE EQUALIZER (2026-11-11, the phones' and TVs' standard): its row
    /// under the output (its page), on that page its switch, the three tone
    /// controls (bass, mids, treble — levels), a preset, and the way to the
    /// whole curve in Controls.
    Eq,
    EqOn,
    EqTone(u8),
    EqPreset(u8),
    EqMore,
    Bright,
    BrightAuto,
    Mic,
    Night,
    Dark,
    Dnd,
}

/// The two levels a hand drags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Volume,
    Bright,
    /// A tone control of the equalizer's page (0 bass, 1 mids, 2 treble).
    Tone(u8),
}

impl Level {
    pub(crate) fn hit(self) -> Hit {
        Hit::Quick(match self {
            Level::Volume => QuickHit::Volume,
            Level::Bright => QuickHit::Bright,
            Level::Tone(i) => QuickHit::EqTone(i),
        })
    }
}

#[derive(Default)]
pub(crate) struct QuickState {
    /// What the hand set lately, and when: shown over the readings for `HOLD`.
    volume: Option<(f32, Instant)>,
    muted: Option<(bool, Instant)>,
    bright: Option<(f32, Instant)>,
    mic_muted: Option<(bool, Instant)>,
    /// The output picked lately (its id).
    output: Option<(u32, Instant)>,
    /// The level a press took hold of, until the release.
    pub(crate) drag: Option<Level>,
    /// The warmth night light had when it was turned off (this run).
    night_was: Option<u32>,
    /// The output page is up (the machine page's own sub-view).
    pub(crate) output_page: bool,
    /// The equalizer's page is up (likewise).
    pub(crate) eq_page: bool,
    /// The default output's equalizer as the hand set it lately (its node).
    eq: Option<((String, crate::eq::Eq), Instant)>,
    /// …and as last read from its file (the row's words; read again a
    /// second later at most).
    eq_read: std::cell::RefCell<Option<(String, crate::eq::Eq, Instant)>>,
    worker: Option<Sender<(&'static str, String)>>,
}

fn held<T: Copy>(v: Option<(T, Instant)>) -> Option<T> {
    v.filter(|(_, at)| at.elapsed() < HOLD).map(|(v, _)| v)
}

/// The thread that carries the picks out: `controls::apply`, the newest of each
/// key only (a drag sends faster than `wpctl` answers).
fn spawn_worker() -> Option<Sender<(&'static str, String)>> {
    let (tx, rx) = mpsc::channel::<(&'static str, String)>();
    let spawned = std::thread::Builder::new().name("quick-settings".into()).spawn(move || {
        let mut zone = crate::controls::ZonePick::default();
        while let Ok(first) = rx.recv() {
            let mut todo = vec![first];
            while let Ok((k, v)) = rx.try_recv() {
                match todo.iter_mut().find(|(kk, _)| *kk == k) {
                    Some(slot) => slot.1 = v,
                    None => todo.push((k, v)),
                }
            }
            for (k, v) in todo {
                let _ = crate::controls::apply(k, &v, &mut zone);
            }
        }
    });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            warn!("quick settings: no thread ({e}); nothing can be set");
            None
        }
    }
}

fn quick(h: QuickHit) -> Hit {
    Hit::Quick(h)
}

/// A row that switches: its symbol, its words, the kit's switch; the whole row
/// is the click.
fn switch_row(glyph: &'static str, title: &str, sub: String, on: bool, hit: Hit) -> Item {
    Item::Row {
        hit,
        more: None,
        tile: Some(glyph),
        title: title.to_owned(),
        sub,
        tone: Tone::Normal,
        trail: Trail::Switch(on),
        sel: false,
    }
}

impl App {
    fn quick_send(&mut self, key: &'static str, value: String) {
        if self.gear.quick.worker.is_none() {
            self.gear.quick.worker = spawn_worker();
        }
        if let Some(tx) = &self.gear.quick.worker {
            let _ = tx.send((key, value));
        }
    }

    // --- what is true now ------------------------------------------------------

    /// The volume 0..1 and whether it is muted (`None`: nothing to play sound
    /// through yet).
    fn quick_volume(&self) -> Option<(f32, bool)> {
        let out = self.brain.as_ref().and_then(|c| c.default_output());
        let read = out.map(|o| (o.volume_pct as f32 / 100.0, o.muted));
        let q = &self.gear.quick;
        let level = held(q.volume).or(read.map(|r| r.0))?;
        let muted = held(q.muted).or(read.map(|r| r.1)).unwrap_or(false);
        Some((level.clamp(0.0, 1.5), muted))
    }

    fn quick_bright(&self) -> Option<f32> {
        held(self.gear.quick.bright).or(self.gear.pages.live.bright)
    }

    fn quick_mic_muted(&self) -> Option<bool> {
        held(self.gear.quick.mic_muted).or(self.gear.pages.live.mic_muted)
    }

    fn night_on(&self) -> bool {
        self.screen.temperature_k.is_some_and(|k| k < crate::screen::NEUTRAL_K)
    }

    /// Whether the sun decides the look now (Auto or Ambient in Colours).
    fn look_follows_sun() -> bool {
        crate::palette::choice().ambient_of(crate::palette::Surface::Shell)
    }

    // --- the page --------------------------------------------------------------

    /// SOUND AND SCREEN, then the MODES of this thread's own: the machine page
    /// puts them after its first two rows and adds its own modes after.
    pub(crate) fn quick_items(&self) -> Vec<Item> {
        let mut items = vec![Item::Heading { text: tr("Sound and screen").into(), busy: false }];
        if let Some((level, muted)) = self.quick_volume() {
            items.push(Item::Level {
                hit: quick(QuickHit::Volume),
                glyph: if muted || level <= 0.0 {
                    G_VOLUME_OFF
                } else if level < 0.4 {
                    G_VOLUME_LOW
                } else {
                    G_VOLUME
                },
                glyph_hit: quick(QuickHit::Mute),
                value: level.min(1.0),
                word: if muted { tr("Muted").to_owned() } else { format!("{:.0}%", level * 100.0) },
                word_hit: Hit::None,
                word_on: false,
                quiet: muted,
            });
            let out = self.brain.as_ref().and_then(|c| c.default_output());
            let picked = held(self.gear.quick.output);
            let name = picked
                .and_then(|id| self.brain.as_ref()?.outputs.iter().find(|o| o.id == id))
                .or(out)
                .map(|o| o.description.clone());
            if let Some(name) = name {
                items.push(Item::Row {
                    hit: quick(QuickHit::Output),
                    more: Some((quick(QuickHit::Output), crate::gear::G_GO)),
                    // (No symbol: its words under the level's track.)
                    tile: Some(""),
                    title: name,
                    sub: String::new(),
                    tone: Tone::Normal,
                    trail: Trail::None,
                    sel: false,
                });
            }
            // The equalizer: what it is set to, its page.
            if let Some((_, _, eq)) = self.quick_eq() {
                items.push(Item::Row {
                    hit: quick(QuickHit::Eq),
                    more: Some((quick(QuickHit::Eq), crate::gear::G_GO)),
                    tile: Some(""),
                    title: tr("Equalizer").to_owned(),
                    sub: tr(eq.summary()).to_owned(),
                    tone: Tone::Normal,
                    trail: Trail::None,
                    sel: false,
                });
            }
        }
        if let Some(level) = self.quick_bright() {
            let auto = crate::gear_pages::autobright_can();
            items.push(Item::Level {
                hit: quick(QuickHit::Bright),
                glyph: G_BRIGHT,
                glyph_hit: Hit::None,
                value: level,
                word: if auto { tr("Auto").to_owned() } else { format!("{:.0}%", level * 100.0) },
                word_hit: if auto { quick(QuickHit::BrightAuto) } else { Hit::None },
                word_on: auto && crate::gear_pages::autobright_on(),
                quiet: false,
            });
        }
        if let Some(muted) = self.quick_mic_muted() {
            items.push(switch_row(
                if muted { G_MIC_OFF } else { G_MIC },
                tr("Microphone"),
                tr(if muted { "Muted for every app" } else { "Apps can hear you" }).to_owned(),
                !muted,
                quick(QuickHit::Mic),
            ));
        }
        items.push(Item::Heading { text: tr("Modes").into(), busy: false });
        items.push(switch_row(
            G_NIGHT,
            tr("Night light"),
            tr("Warmer colours, easier on the eyes").to_owned(),
            self.night_on(),
            quick(QuickHit::Night),
        ));
        items.push(switch_row(
            G_DARK,
            tr("Dark mode"),
            if Self::look_follows_sun() { tr("Follows the sun now").to_owned() } else { String::new() },
            crate::look::dark(),
            quick(QuickHit::Dark),
        ));
        items.push(switch_row(
            G_DND,
            tr("Do not disturb"),
            tr("Notifications wait in the bell").to_owned(),
            self.notif_muted(),
            quick(QuickHit::Dnd),
        ));
        items
    }

    /// Where the sound plays: every output, the one in use ticked.
    pub(crate) fn quick_output_view(&self) -> View {
        let outputs = self.brain.as_ref().map(|c| c.outputs.clone()).unwrap_or_default();
        let picked = held(self.gear.quick.output);
        let mut items = vec![Item::Heading { text: tr("Sound plays through").into(), busy: false }];
        if outputs.is_empty() {
            items.push(Item::Empty(tr("Nothing to play sound through").into()));
        }
        for o in outputs {
            let on = picked.map_or(o.is_default, |id| id == o.id);
            let d = o.description.to_lowercase();
            let glyph = if d.contains("hdmi") || d.contains("displayport") {
                G_SCREEN
            } else if d.contains("head") || d.contains("buds") || d.contains("pods") {
                G_HEADPHONES
            } else {
                G_SPEAKER
            };
            items.push(Item::Row {
                hit: quick(QuickHit::OutputPick(o.id)),
                more: None,
                tile: Some(glyph),
                title: o.description.clone(),
                sub: String::new(),
                tone: Tone::Normal,
                trail: if on { Trail::Glyph(G_CHECK) } else { Trail::None },
                sel: false,
            });
        }
        View {
            strip: vec![crate::gear::btn(Hit::Back, crate::gear::G_BACK)],
            items,
            ..View::default()
        }
    }

    /// The default output's node name and id, and its equalizer — as the
    /// hand set it lately, else as kept.
    fn quick_eq(&self) -> Option<(String, u32, crate::eq::Eq)> {
        let out = self.brain.as_ref().and_then(|c| c.default_output())?;
        let (node, id) = (out.name.clone(), out.id);
        if let Some(((n, eq), _)) = self.gear.quick.eq.as_ref().filter(|(_, at)| at.elapsed() < HOLD) {
            if *n == node {
                return Some((node, id, eq.clone()));
            }
        }
        let mut read = self.gear.quick.eq_read.borrow_mut();
        let fresh = read.as_ref().is_some_and(|(n, _, at)| *n == node && at.elapsed() < Duration::from_secs(1));
        if !fresh {
            *read = Some((node.clone(), crate::eq::load(&node), Instant::now()));
        }
        read.as_ref().map(|(_, eq, _)| (node, id, eq.clone()))
    }

    /// THE EQUALIZER'S PAGE: on or off, the three tone controls, the presets
    /// (the one in use ticked), and the whole curve in Controls.
    pub(crate) fn quick_eq_view(&self) -> View {
        let mut items = Vec::new();
        let Some((_, _, eq)) = self.quick_eq() else {
            items.push(Item::Empty(tr("Nothing to play sound through").into()));
            return View { strip: vec![crate::gear::btn(Hit::Back, crate::gear::G_BACK)], items, ..View::default() };
        };
        let name = self.brain.as_ref().and_then(|c| c.default_output()).map(|o| o.description.clone()).unwrap_or_default();
        items.push(Item::Heading { text: name, busy: false });
        items.push(Item::Row {
            hit: quick(QuickHit::EqOn),
            more: None,
            tile: Some(""),
            title: tr("Equalizer").to_owned(),
            sub: tr(eq.summary()).to_owned(),
            tone: Tone::Normal,
            trail: Trail::Switch(eq.on),
            sel: false,
        });
        for (i, (g, word)) in eq.tone().into_iter().zip(["Bass", "Mids", "Treble"]).enumerate() {
            let level = if g.abs() < 0.05 { "0".to_owned() } else { format!("{}{}", if g > 0.0 { "+" } else { "−" }, (g.abs() * 10.0).round() / 10.0) };
            items.push(Item::Level {
                hit: quick(QuickHit::EqTone(i as u8)),
                glyph: "",
                glyph_hit: Hit::None,
                value: (g + crate::eq::G_MAX) / (2.0 * crate::eq::G_MAX),
                word: format!("{} {level}", tr(word)),
                word_hit: Hit::None,
                word_on: false,
                quiet: !eq.on,
            });
        }
        items.push(Item::Heading { text: tr("Presets").into(), busy: false });
        for (i, (preset, _)) in crate::eq::PRESETS.iter().enumerate() {
            items.push(Item::Row {
                hit: quick(QuickHit::EqPreset(i as u8)),
                more: None,
                tile: Some(""),
                title: tr(preset).to_owned(),
                sub: String::new(),
                tone: Tone::Normal,
                trail: if eq.on && eq.preset() == Some(*preset) { Trail::Glyph(G_CHECK) } else { Trail::None },
                sel: false,
            });
        }
        items.push(Item::Row {
            hit: quick(QuickHit::EqMore),
            more: Some((quick(QuickHit::EqMore), crate::gear::G_GO)),
            tile: Some(""),
            title: tr("More…").to_owned(),
            sub: tr("The whole curve, band by band").to_owned(),
            tone: Tone::Normal,
            trail: Trail::None,
            sel: false,
        });
        View {
            strip: vec![crate::gear::btn(Hit::Back, crate::gear::G_BACK)],
            items,
            ..View::default()
        }
    }

    /// The hand changed the default output's equalizer: shown at once, kept
    /// and heard through the quick thread (`eq_page::apply`, "eq-quick").
    fn quick_eq_set(&mut self, f: impl FnOnce(&mut crate::eq::Eq), what: &str, arg: String) {
        let Some((node, id, mut eq)) = self.quick_eq() else { return };
        f(&mut eq);
        eq.settle();
        self.gear.quick.eq = Some(((node.clone(), eq), Instant::now()));
        self.quick_send("eq-quick", format!("{node}|{id}|{what}|{arg}"));
        self.gear_changed();
    }

    // --- what a click does -----------------------------------------------------

    pub(crate) fn quick_settings_click(&mut self, hit: QuickHit) {
        match hit {
            QuickHit::Volume => self.quick_level_to_ptr(Level::Volume),
            QuickHit::Bright => self.quick_level_to_ptr(Level::Bright),
            QuickHit::Mute => {
                let muted = self.quick_volume().is_some_and(|(_, m)| m);
                self.gear.quick.muted = Some((!muted, Instant::now()));
                self.quick_send("mute", if muted { "off" } else { "on" }.into());
            }
            QuickHit::Output => {
                self.gear.quick.output_page = true;
                self.gear.arm = None;
                self.gear_show_view();
            }
            QuickHit::OutputPick(id) => {
                info!("quick settings: sound through {id}");
                self.gear.quick.output = Some((id, Instant::now()));
                self.quick_send("sink", id.to_string());
            }
            QuickHit::Eq => {
                self.gear.quick.eq_page = true;
                self.gear.arm = None;
                self.gear_show_view();
            }
            QuickHit::EqOn => {
                let on = !self.quick_eq().is_some_and(|(_, _, eq)| eq.on);
                self.quick_eq_set(|eq| eq.on = on, "on", if on { "on" } else { "off" }.into());
            }
            QuickHit::EqTone(i) => self.quick_level_to_ptr(Level::Tone(i)),
            QuickHit::EqPreset(i) => {
                if let Some((name, _)) = crate::eq::PRESETS.get(usize::from(i)) {
                    self.quick_eq_set(|eq| eq.set_preset(name), "preset", (*name).to_owned());
                }
            }
            QuickHit::EqMore => {
                self.set_stats_box(false);
                self.open_control("Equalizer");
            }
            QuickHit::BrightAuto => self.sys_click(crate::gear_pages::SysHit::AutoBright),
            QuickHit::Mic => {
                let muted = self.quick_mic_muted().unwrap_or(false);
                self.gear.quick.mic_muted = Some((!muted, Instant::now()));
                self.quick_send("mic-mute", if muted { "off" } else { "on" }.into());
            }
            QuickHit::Night => {
                if self.night_on() {
                    self.gear.quick.night_was = self.screen.temperature_k;
                    self.set_screen_temperature(crate::screen::NEUTRAL_K);
                } else {
                    let k = self.gear.quick.night_was.unwrap_or(NIGHT_K);
                    self.set_screen_temperature(k);
                }
            }
            QuickHit::Dark => {
                let dark = !crate::look::dark();
                self.colors_dark_all(dark);
            }
            QuickHit::Dnd => self.toggle_notif_mute(),
        }
        self.gear_changed();
    }

    /// Back from the output page; whether it was up.
    pub(crate) fn quick_back(&mut self) -> bool {
        let output = std::mem::take(&mut self.gear.quick.output_page);
        let eq = std::mem::take(&mut self.gear.quick.eq_page);
        output || eq
    }

    // --- a level in the hand ---------------------------------------------------

    fn quick_level_set(&mut self, level: Level, v: f32) {
        let v = v.clamp(0.0, 1.0);
        let now = Instant::now();
        match level {
            Level::Volume => {
                self.gear.quick.volume = Some((v, now));
                // (Setting a level is wanting to hear it: `apply` unmutes.)
                self.gear.quick.muted = Some((false, now));
                self.quick_send("volume", format!("{v:.3}"));
            }
            Level::Bright => {
                // (Never quite dark: a screen at 0 shows nothing to set it back with.)
                let v = v.max(0.02);
                self.gear.quick.bright = Some((v, now));
                self.quick_send("brightness", format!("{:.0}", v * 100.0));
            }
            // A tone control: −12…+12 dB, in half dB.
            Level::Tone(i) => {
                let g = ((v * 2.0 - 1.0) * crate::eq::G_MAX * 2.0).round() / 2.0;
                self.quick_eq_set(|eq| eq.set_tone(usize::from(i), g), "tone", format!("{i}:{g}"));
                return;
            }
        }
        self.gear_changed();
    }

    fn quick_level_to_ptr(&mut self, level: Level) {
        if let Some(v) = self.gear_level_at_ptr(&level.hit()) {
            self.quick_level_set(level, v);
        }
    }

    /// A left press on the banner: on a level's track it takes hold of it
    /// (a drag follows the pointer until the release). Whether it did.
    pub(crate) fn quick_press(&mut self) -> bool {
        let level = match self.gear_hit_now() {
            Hit::Quick(QuickHit::Volume) => Level::Volume,
            Hit::Quick(QuickHit::Bright) => Level::Bright,
            Hit::Quick(QuickHit::EqTone(i)) => Level::Tone(i),
            _ => return false,
        };
        self.gear.quick.drag = Some(level);
        self.quick_level_to_ptr(level);
        true
    }

    /// The pointer moved: a level in the hand follows it.
    pub(crate) fn quick_motion(&mut self) {
        if let Some(level) = self.gear.quick.drag {
            self.quick_level_to_ptr(level);
        }
    }

    /// The release ends the hold (the click that follows sets the level once more).
    pub(crate) fn quick_release(&mut self) {
        self.gear.quick.drag = None;
    }

    /// The wheel over a level: a notch is `NOTCH` of it, up = more. Whether it
    /// was over one.
    pub(crate) fn quick_wheel(&mut self, value: f32) -> bool {
        let (level, now) = match self.gear_hit_now() {
            Hit::Quick(QuickHit::Volume | QuickHit::Mute) => (Level::Volume, self.quick_volume().map(|v| v.0)),
            Hit::Quick(QuickHit::Bright | QuickHit::BrightAuto) => (Level::Bright, self.quick_bright()),
            Hit::Quick(QuickHit::EqTone(i)) => {
                let g = self.quick_eq().map(|(_, _, eq)| eq.tone()[usize::from(i).min(2)]);
                (Level::Tone(i), g.map(|g| (g + crate::eq::G_MAX) / (2.0 * crate::eq::G_MAX)))
            }
            _ => return false,
        };
        let Some(now) = now else { return true };
        self.quick_level_set(level, now.min(1.0) - value / NOTCH_UNITS * NOTCH);
        true
    }
}

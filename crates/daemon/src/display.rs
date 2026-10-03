//! Live display settings: a screen's **scale** and **resolution**, changed
//! from the control panel and applied at once — no rebuild, no config reload
//! (Max, 2026-10-01: these settings "dont really need a rebuild … i want it
//! to happen live").
//!
//! Three files, one job each:
//!
//! - **Golem's defaults** stay in the system's `hyprland.lua` (immutable,
//!   updated with the system).
//! - **The owner's choices** live in `~/.config/golem/settings.json`, the one
//!   settings file every consumer reads. This module owns its `displays` key
//!   and writes every other key back as it found it.
//! - **The compositor's copy** is `~/.config/golem/settings.lua`, generated
//!   here from the JSON. Golem's `hyprland.lua` runs it last, at compositor
//!   start and on every reload, so a choice is on the screen from the first
//!   frame and does not need this daemon to put it there.
//!
//! A change is put on the screen with the very rule the file holds
//! (`hl.monitor` through `hyprctl eval`), then saved. Nothing is re-asserted
//! from here afterwards: the compositor keeps a monitor rule until its config
//! is read again, and reading the config runs the file.
//!
//! **A scale** can only make things larger or smaller, so it is saved as it
//! is set. **A resolution** can leave the screen black, so it is only tried:
//! it goes back by itself after [`KEEP_SECS`] unless the owner keeps it, and
//! it is not written anywhere until then — a mode the panel cannot show never
//! reaches the file the next login reads.
//!
//! A screen is known by its description (`desc:Maker Model Serial`), the same
//! selector Golem's own rules use, so a choice follows the panel across ports.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{hypr, App};

/// How long a tried resolution stays up before it goes back by itself.
const KEEP_SECS: u64 = 15;
/// How long the compositor is given to put a rule on the screen before the
/// panel reads the screen back (a rule lands with the next rendered frame).
const SETTLE: Duration = Duration::from_millis(350);

/// The pointer is put back this long after a change of scale at the latest
/// (normally as soon as our own surface is told the new scale).
const POINTER_DEADLINE: Duration = Duration::from_millis(150);

/// The compositor counts a scale in 120ths (the fractional-scale protocol).
const SCALE_UNIT: f64 = 120.0;
/// The scales the panel offers: from this one up…
const OFFER_MIN: f64 = 0.75;
/// …while the screen still looks at least this large (long side, short
/// side), so the panel that set a scale can always be reached to undo it.
const OFFER_LOGICAL_MIN: (f64, f64) = (960.0, 540.0);
/// Offered scales are at least this far apart (as a ratio): 1.042 next to
/// 1.0 is not a choice anyone can see.
const OFFER_GAP: f64 = 1.075;
/// Any scale a rule may carry at all (the `display` verb, the saved file).
const SCALE_RANGE: (f64, f64) = (0.5, 3.0);

/// A video mode: pixels and refresh rate (`0.0` = whatever the panel picks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mode {
    pub w: u32,
    pub h: u32,
    pub hz: f64,
}

impl Mode {
    /// Read `1440x900@60.00Hz` (the compositor's), `1440x900@60` or
    /// `1440x900`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_suffix("Hz").unwrap_or(text);
        let (size, hz) = match text.split_once('@') {
            Some((size, hz)) => (size, hz.parse::<f64>().ok()?),
            None => (text, 0.0),
        };
        let (w, h) = size.split_once('x')?;
        let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
        (w > 0 && h > 0 && hz.is_finite() && (0.0..=1000.0).contains(&hz)).then_some(Self { w, h, hz })
    }

    /// As a monitor rule's `mode`. Built from the numbers, never from the
    /// text it was read from: this is what reaches the generated Lua.
    pub(crate) fn rule(&self) -> String {
        if self.hz > 0.0 {
            format!("{}x{}@{:.2}", self.w, self.h, self.hz)
        } else {
            format!("{}x{}", self.w, self.h)
        }
    }

    fn same_size(&self, other: &Mode) -> bool {
        self.w == other.w && self.h == other.h
    }

    fn same(&self, other: &Mode) -> bool {
        self.same_size(other) && (self.hz - other.hz).abs() < 0.5
    }
}

/// A connected screen, as the compositor reports it.
#[derive(Debug, Clone)]
pub(crate) struct Monitor {
    /// Connector (`eDP-1`).
    pub name: String,
    /// `Maker Model Serial`: what the screen is known by.
    pub desc: String,
    pub model: String,
    pub mode: Mode,
    pub scale: f64,
    pub modes: Vec<Mode>,
    pub focused: bool,
    /// Its top-left corner among the screens, in logical pixels.
    pub pos: (f64, f64),
}

impl Monitor {
    /// The rule selector this screen's choice is saved under.
    pub(crate) fn selector(&self) -> String {
        if self.desc.is_empty() {
            self.name.clone()
        } else {
            format!("desc:{}", self.desc)
        }
    }

    /// What to call it: a laptop's own panel has a model nobody knows it by.
    pub(crate) fn title(&self) -> String {
        let built_in = ["eDP", "LVDS", "DSI"].iter().any(|p| self.name.starts_with(p));
        if built_in {
            "Built-in screen".to_owned()
        } else if self.model.is_empty() {
            self.name.clone()
        } else {
            self.model.clone()
        }
    }
}

/// Read the compositor's `j/monitors` reply.
pub(crate) fn parse_monitors(json: &serde_json::Value) -> Vec<Monitor> {
    json.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let (w, h) = (m["width"].as_u64()?, m["height"].as_u64()?);
            let mode = Mode {
                w: u32::try_from(w).ok()?,
                h: u32::try_from(h).ok()?,
                hz: m["refreshRate"].as_f64().unwrap_or(0.0),
            };
            let mut modes: Vec<Mode> = m["availableModes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| Mode::parse(t.as_str()?))
                .collect();
            if !modes.iter().any(|x| x.same(&mode)) {
                modes.push(mode);
            }
            // The reply rounds the scale to two places (1.125 reads 1.13):
            // the real one is the clean scale it stands for.
            let reported = m["scale"].as_f64().unwrap_or(1.0);
            let scale = nearest_clean(mode.w, mode.h, reported)
                .filter(|s| (s - reported).abs() < 0.006)
                .unwrap_or(reported);
            Some(Monitor {
                name: m["name"].as_str()?.to_owned(),
                desc: m["description"].as_str().unwrap_or("").trim().to_owned(),
                model: m["model"].as_str().unwrap_or("").trim().to_owned(),
                mode,
                scale,
                modes,
                focused: m["focused"].as_bool().unwrap_or(false),
                pos: (m["x"].as_f64().unwrap_or(0.0), m["y"].as_f64().unwrap_or(0.0)),
            })
        })
        .collect()
}

/// The connected screens, or none when the compositor does not answer.
fn monitors() -> Vec<Monitor> {
    hypr::monitors_json().map(|json| parse_monitors(&json)).unwrap_or_default()
}

/// Every scale, in 120ths, that divides a `w`×`h` panel into whole logical
/// pixels — the only ones the compositor takes as given (any other it moves
/// to the nearest of these and complains).
fn clean_units(w: u32, h: u32) -> Vec<u32> {
    let (lo, hi) = ((SCALE_RANGE.0 * SCALE_UNIT) as u32, (SCALE_RANGE.1 * SCALE_UNIT) as u32);
    let (pw, ph) = (u64::from(w) * 120, u64::from(h) * 120);
    (lo..=hi).filter(|&k| pw.is_multiple_of(u64::from(k)) && ph.is_multiple_of(u64::from(k))).collect()
}

/// The clean scale nearest `want` for a `w`×`h` panel.
pub(crate) fn nearest_clean(w: u32, h: u32, want: f64) -> Option<f64> {
    clean_units(w, h)
        .into_iter()
        .map(|k| f64::from(k) / SCALE_UNIT)
        .min_by(|a, b| (a - want).abs().total_cmp(&(b - want).abs()))
}

/// The scales the panel offers for a `w`×`h` panel now at `current`: clean,
/// not so large the panel itself no longer fits, and far enough apart to
/// tell apart — the round ones first when two crowd each other. `current`
/// is always among them.
pub(crate) fn scale_stops(w: u32, h: u32, current: f64) -> Vec<f64> {
    let current = (current * SCALE_UNIT).round() as u32;
    let (long, short) = (f64::from(w.max(h)), f64::from(w.min(h)));
    let offered = |k: u32| {
        let s = f64::from(k) / SCALE_UNIT;
        // 100 % (the panel's own pixels) always: whatever was chosen, the way
        // home is one click — even on a panel too small for the size rule.
        k == current || k == SCALE_UNIT as u32 || (s >= OFFER_MIN && long / s >= OFFER_LOGICAL_MIN.0 && short / s >= OFFER_LOGICAL_MIN.1)
    };
    // Whole and half scales, then quarters, then eighths, then the rest.
    let rank = |k: u32| {
        if k == current {
            0
        } else {
            [60, 30, 15].iter().position(|&step| k.is_multiple_of(step)).map_or(4, |i| i + 1)
        }
    };
    let mut units: Vec<u32> = clean_units(w, h).into_iter().filter(|&k| offered(k)).collect();
    units.sort_by_key(|&k| (rank(k), k));
    let mut kept: Vec<u32> = Vec::new();
    for k in units {
        if kept.iter().all(|&j| f64::from(k.max(j)) / f64::from(k.min(j)) >= OFFER_GAP) {
            kept.push(k);
        }
    }
    // ALWAYS a choice (Max, 2026-10-02): a panel whose width has a large
    // prime factor has almost no clean scales — 1366×768 (1366 = 2·683)
    // allows only 50, 67, 100 and 200 %, and the rules above left 100 %
    // alone, so the ASUS showed no Scale options at all. Then offer the
    // nearest clean scale on each side of the current one, the smaller even
    // under the usual floor, the larger only if the panel still fits.
    // Each side prefers the nearest stop far enough away to see, else the
    // nearest at all.
    if kept.len() < 2 {
        let clean = clean_units(w, h);
        let apart = |k: u32| f64::from(k.max(current)) / f64::from(k.min(current)) >= OFFER_GAP;
        let below: Vec<u32> = clean.iter().copied().filter(|&k| k < current).collect();
        let above: Vec<u32> = clean.iter().copied().filter(|&k| k > current && offered(k)).collect();
        if let Some(k) = below.iter().copied().filter(|&k| apart(k)).max().or_else(|| below.iter().copied().max()) {
            kept.push(k);
        }
        if let Some(k) = above.iter().copied().filter(|&k| apart(k)).min() {
            kept.push(k);
        }
    }
    // The way home survives the crowding rule: from 101.7 % (a 976×549
    // panel) the 100 % next to it is still the one to offer.
    let home = SCALE_UNIT as u32;
    if !kept.contains(&home) && clean_units(w, h).contains(&home) {
        kept.retain(|&k| k == current || f64::from(k.max(home)) / f64::from(k.min(home)) >= OFFER_GAP);
        kept.push(home);
    }
    kept.sort_unstable();
    kept.into_iter().map(|k| f64::from(k) / SCALE_UNIT).collect()
}

/// Where the pointer belongs after a change of scale so that it has not
/// moved on the glass.
///
/// The compositor keeps the pointer's LOGICAL position through the change,
/// and a logical pixel is exactly what changes size: left alone the pointer
/// leaps toward or away from the screen's corner by the ratio of the two
/// scales — off the very pill that was just clicked. `cursor` and `origin`
/// (the screen's corner) are global logical positions before the change;
/// `px` is the screen in pixels. `None` when the pointer is on another
/// screen.
fn pointer_after(cursor: (f64, f64), origin: (f64, f64), px: (u32, u32), old: f64, new: f64) -> Option<(f64, f64)> {
    let (dx, dy) = (cursor.0 - origin.0, cursor.1 - origin.1);
    let inside = dx >= 0.0 && dy >= 0.0 && dx * old < f64::from(px.0) && dy * old < f64::from(px.1);
    inside.then(|| (origin.0 + dx * old / new, origin.1 + dy * old / new))
}

/// A pointer to put back once the compositor has made a change of scale.
pub(crate) struct PointerFix {
    to: (f64, f64),
    /// The scale being waited for, in 120ths.
    scale120: u32,
}

/// What the owner chose for one screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DisplayChoice {
    /// `WxH@Hz`.
    pub mode: String,
    pub scale: f64,
    /// Where it sits among the screens (`auto`, `0x0`…). Nothing sets it
    /// yet; absent means `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// `~/.config/golem/settings.json`: the owner's live settings. Only
/// `displays` is this module's; the rest is carried through untouched.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GolemSettings {
    /// Rule selector (`desc:…`) → the choice for that screen.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub displays: BTreeMap<String, DisplayChoice>,
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

/// A file in Golem's own config directory (`$XDG_CONFIG_HOME/golem/`).
fn golem_path(file_name: &str) -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("golem")
        .join(file_name)
}

impl GolemSettings {
    /// Read the file as it is now. Always fresh, never cached: other parts
    /// of Golem write their own keys to it.
    pub(crate) fn load() -> Self {
        crate::persist::read_json(&golem_path("settings.json")).unwrap_or_default()
    }

    /// Write the file, and the compositor's copy beside it.
    fn save(&self) {
        crate::persist::write_json("golem settings", &golem_path("settings.json"), self);
        crate::persist::write_text("golem settings", &golem_path("settings.lua"), &self.lua());
    }

    /// The compositor's copy: the settings as the Lua that Golem's
    /// `hyprland.lua` runs last. An entry that does not read as a mode, a
    /// scale and a place is left out rather than passed on.
    pub(crate) fn lua(&self) -> String {
        let mut out = String::from(
            "-- Golem live settings, generated from settings.json by the control panel.\n\
             -- Do not edit: it is rewritten on every change. Golem's hyprland.lua runs\n\
             -- it last, so what is here wins over the system's defaults.\n",
        );
        for (selector, choice) in &self.displays {
            match monitor_rule(selector, choice) {
                Some(rule) => {
                    out.push_str(&rule);
                    out.push('\n');
                }
                None => warn!("golem settings: leaving out the display {selector:?} ({choice:?})"),
            }
        }
        out
    }
}

/// A string as a Lua literal.
fn lua_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `auto`, `auto-left`…, or `XxY`.
fn valid_position(text: &str) -> bool {
    let auto = text.starts_with("auto") && text.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    let at = text
        .split_once('x')
        .is_some_and(|(x, y)| x.parse::<i32>().is_ok() && y.parse::<i32>().is_ok());
    auto || at
}

/// The monitor rule for a choice — the one line both the live change and
/// the saved file use. Complete on purpose: a rule under a new selector
/// starts from the compositor's blank rule (1280×720 at 0,0), not from the
/// one it overrides.
fn monitor_rule(selector: &str, choice: &DisplayChoice) -> Option<String> {
    let mode = Mode::parse(&choice.mode)?;
    let scale = choice.scale;
    if selector.is_empty() || !(SCALE_RANGE.0..=SCALE_RANGE.1).contains(&scale) {
        return None;
    }
    let position = choice.position.as_deref().unwrap_or("auto");
    if !valid_position(position) {
        return None;
    }
    Some(format!(
        "hl.monitor({{ output = {}, mode = \"{}\", position = \"{position}\", scale = {scale} }})",
        lua_quote(selector),
        mode.rule(),
    ))
}

/// Put a choice on the screen now.
fn apply(selector: &str, choice: &DisplayChoice) -> bool {
    match monitor_rule(selector, choice) {
        Some(rule) => {
            let ok = hypr::eval_ok(&rule);
            if !ok {
                warn!("display: the compositor refused {rule}");
            }
            ok
        }
        None => {
            warn!("display: not a rule: {selector:?} {choice:?}");
            false
        }
    }
}

/// A resolution being tried: on the screen, not saved.
pub(crate) struct Pending {
    selector: String,
    /// What was on the screen before, to go back to.
    previous: DisplayChoice,
    wanted: DisplayChoice,
    deadline: Instant,
}

/// What an open display setting shows (see `panel.rs`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisplayView {
    pub screen: String,
    pub mode: Mode,
    pub scale: f64,
    /// The scales on offer, ascending.
    pub stops: Vec<f64>,
    /// The resolutions on offer, largest first.
    pub sizes: Vec<(u32, u32)>,
    /// The refresh rates of the current resolution, ascending.
    pub rates: Vec<f64>,
}

impl DisplayView {
    fn of(m: &Monitor) -> Self {
        let mut sizes: Vec<(u32, u32)> = m.modes.iter().map(|x| (x.w, x.h)).collect();
        sizes.sort_unstable_by(|a, b| (u64::from(b.0) * u64::from(b.1), b.0).cmp(&(u64::from(a.0) * u64::from(a.1), a.0)));
        sizes.dedup();
        let mut rates: Vec<f64> = m.modes.iter().filter(|x| x.same_size(&m.mode)).map(|x| x.hz).collect();
        rates.sort_by(f64::total_cmp);
        rates.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        Self {
            screen: m.title(),
            mode: m.mode,
            scale: m.scale,
            stops: scale_stops(m.mode.w, m.mode.h, m.scale),
            sizes,
            rates,
        }
    }

    /// The size the screen looks like at its scale.
    pub(crate) fn looks_like(&self) -> (u32, u32) {
        let logical = |px: u32| (f64::from(px) / self.scale).round() as u32;
        (logical(self.mode.w), logical(self.mode.h))
    }
}

impl App {
    /// The screen a display setting is about: the one in use.
    fn display_target(&self) -> Option<Monitor> {
        let mut all = monitors();
        match all.iter().position(|m| m.focused) {
            Some(i) => Some(all.swap_remove(i)),
            None => all.into_iter().next(),
        }
    }

    /// What the open display setting shows, read from the screen now.
    pub(crate) fn display_view(&self) -> Option<DisplayView> {
        self.display_target().as_ref().map(DisplayView::of)
    }

    /// Seconds a tried resolution still has, if one is being tried.
    pub(crate) fn display_keep_left(&self) -> Option<f32> {
        self.display_pending
            .as_ref()
            .map(|p| p.deadline.saturating_duration_since(Instant::now()).as_secs_f32())
    }

    /// The compositor needs a moment to put a rule on the screen: have the
    /// panel read the screen back once it has.
    fn display_changed(&mut self) {
        self.settings_panel_mut().display_refresh_at(Instant::now() + SETTLE);
        self.schedule_frame();
    }

    /// Set the screen's scale (to the nearest one that divides it cleanly),
    /// live, and save it. The change is made under a dissolve (see
    /// [`crate::transition`]): every client redraws at the new scale on its
    /// own frame, and shown bare that reads as the screen jumping.
    pub(crate) fn set_display_scale(&mut self, want: f64) {
        let Some(m) = self.display_target() else {
            warn!("display: no screen to scale");
            return;
        };
        let Some(scale) = nearest_clean(m.mode.w, m.mode.h, want) else {
            warn!("display: {}x{} has no clean scale near {want}", m.mode.w, m.mode.h);
            return;
        };
        if (scale - m.scale).abs() < 1e-4 {
            return;
        }
        let selector = m.selector();
        let position = GolemSettings::load().displays.get(&selector).and_then(|c| c.position.clone());
        let choice = DisplayChoice { mode: m.mode.rule(), scale, position };
        // The pill asked for is the lit one from the first frame drawn at
        // the new scale, not once the screen has been read back.
        self.settings_panel_mut().display_expect_scale(scale);
        let (name, old) = (m.name.clone(), m.scale);
        let (origin, px) = (m.pos, (m.mode.w, m.mode.h));
        let output = name.clone();
        self.dissolve(
            &output,
            scale,
            Box::new(move |app: &mut App| {
                // Where the pointer is on the glass, read at the last moment.
                let pointer = hypr::cursor_pos().and_then(|c| pointer_after(c, origin, px, old, choice.scale));
                let scale120 = (choice.scale * SCALE_UNIT).round() as u32;
                if app.apply_display_scale(&name, &selector, choice, old) {
                    app.display_pointer = pointer.map(|to| PointerFix { to, scale120 });
                    // The cue is our own surface being told the new scale;
                    // should that never come, put the pointer back anyway.
                    let armed = app.loop_handle.insert_source(Timer::from_duration(POINTER_DEADLINE), |_, _, app: &mut App| {
                        app.display_pointer_back(None);
                        TimeoutAction::Drop
                    });
                    if armed.is_err() {
                        app.display_pointer_back(None);
                    }
                }
            }),
        );
    }

    /// The compositor has made a change of scale (`scale120`: our surface
    /// was just told so; `None`: time is up): the pointer goes back to
    /// where it was on the glass.
    pub(crate) fn display_pointer_back(&mut self, scale120: Option<u32>) {
        if scale120.is_some_and(|s| self.display_pointer.as_ref().is_some_and(|f| f.scale120 != s)) {
            return;
        }
        if let Some(fix) = self.display_pointer.take() {
            hypr::move_cursor(fix.to.0, fix.to.1);
        }
    }

    /// Put a scale on the screen and save it. Returns whether the
    /// compositor took it.
    fn apply_display_scale(&mut self, name: &str, selector: &str, choice: DisplayChoice, old: f64) -> bool {
        let applied = apply(selector, &choice);
        if applied {
            info!("display: {name} scale {old} -> {}", choice.scale);
            match &mut self.display_pending {
                // On a resolution still being tried: part of the trial.
                Some(p) if p.selector == selector => p.wanted = choice,
                _ => {
                    let mut store = GolemSettings::load();
                    store.displays.insert(selector.to_owned(), choice);
                    store.save();
                }
            }
        }
        // Read the screen back either way: it confirms the change, or puts
        // the lit pill back when the compositor refused it.
        self.display_changed();
        applied
    }

    /// Try a resolution, live. It goes back by itself unless kept
    /// ([`Self::keep_display`]). `hz`: the refresh rate wanted, else the one
    /// nearest the current.
    pub(crate) fn set_display_mode(&mut self, w: u32, h: u32, hz: Option<f64>) {
        let Some(m) = self.display_target() else {
            warn!("display: no screen to set");
            return;
        };
        let want_hz = hz.unwrap_or(m.mode.hz);
        let Some(mode) = m
            .modes
            .iter()
            .filter(|x| x.w == w && x.h == h)
            .min_by(|a, b| (a.hz - want_hz).abs().total_cmp(&(b.hz - want_hz).abs()))
            .copied()
        else {
            warn!("display: {} has no {w}x{h} mode", m.name);
            return;
        };
        if mode.same(&m.mode) {
            return;
        }
        let selector = m.selector();
        let saved = GolemSettings::load().displays.remove(&selector);
        let position = saved.and_then(|c| c.position);
        let wanted = DisplayChoice {
            mode: mode.rule(),
            // The same scale where the new size divides by it, else the
            // nearest that does.
            scale: nearest_clean(mode.w, mode.h, m.scale).unwrap_or(1.0),
            position: position.clone(),
        };
        // A second try on top of a first goes back to before the first.
        let previous = match self.display_pending.take() {
            Some(p) if p.selector == selector => p.previous,
            _ => DisplayChoice { mode: m.mode.rule(), scale: m.scale, position },
        };
        if !apply(&selector, &wanted) {
            return;
        }
        info!("display: {} trying {} (was {}), {KEEP_SECS} s to keep it", m.name, wanted.mode, previous.mode);
        let deadline = Instant::now() + Duration::from_secs(KEEP_SECS);
        self.display_pending = Some(Pending { selector, previous, wanted, deadline });
        let timer = Timer::from_duration(Duration::from_secs(KEEP_SECS));
        let armed = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if app.display_pending.as_ref().is_some_and(|p| p.deadline == deadline) {
                info!("display: not kept in time");
                app.revert_display();
            }
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            // No way to go back by itself: do not leave it up.
            warn!("display: cannot arm the keep timer ({e}), going back now");
            self.revert_display();
            return;
        }
        self.display_changed();
    }

    /// Keep the resolution being tried: save it.
    pub(crate) fn keep_display(&mut self) {
        let Some(p) = self.display_pending.take() else {
            return;
        };
        info!("display: keeping {}", p.wanted.mode);
        let mut store = GolemSettings::load();
        store.displays.insert(p.selector, p.wanted);
        store.save();
        self.display_changed();
    }

    /// Give up the resolution being tried: back to what was there.
    pub(crate) fn revert_display(&mut self) {
        let Some(p) = self.display_pending.take() else {
            return;
        };
        info!("display: back to {}", p.previous.mode);
        apply(&p.selector, &p.previous);
        self.display_changed();
    }

    /// Forget the owner's choice for the screen in use: back to Golem's own.
    /// The compositor has no way to drop one rule, so its config is re-read.
    pub(crate) fn reset_display(&mut self) {
        let Some(m) = self.display_target() else {
            return;
        };
        self.display_pending = None;
        let mut store = GolemSettings::load();
        if store.displays.remove(&m.selector()).is_some() {
            store.save();
        }
        info!("display: {} back to Golem's defaults", m.name);
        hypr::reload_config();
        self.display_changed();
    }

    /// `waverunner-ctl display …`: the settings without the panel (scripts,
    /// key bindings, and the way to see them work with no pointer).
    ///
    /// `scale <n>` · `mode <WxH[@Hz]>` · `keep` · `back` · `reset` ·
    /// `show scale|resolution` (open the panel on that setting) · nothing:
    /// say what is on the screen.
    pub(crate) fn display_command(&mut self, args: &str) {
        let mut words = args.split_whitespace();
        match (words.next(), words.next()) {
            (Some("scale"), Some(n)) => match n.parse::<f64>() {
                Ok(scale) if scale.is_finite() => self.set_display_scale(scale),
                _ => warn!("display: not a scale: {n:?}"),
            },
            (Some("mode"), Some(text)) => match Mode::parse(text) {
                Some(mode) => self.set_display_mode(mode.w, mode.h, (mode.hz > 0.0).then_some(mode.hz)),
                None => warn!("display: not a mode: {text:?}"),
            },
            (Some("keep"), None) => self.keep_display(),
            (Some("back"), None) => self.revert_display(),
            (Some("reset"), None) => self.reset_display(),
            (Some("show"), Some(which)) => match which {
                "scale" => self.show_control("Scale"),
                "resolution" => self.show_control("Resolution"),
                _ => warn!("display: no setting called {which:?}"),
            },
            (None, _) => {
                for m in monitors() {
                    let stops = scale_stops(m.mode.w, m.mode.h, m.scale);
                    let modes: Vec<String> = m.modes.iter().map(Mode::rule).collect();
                    info!(
                        "display: {} ({}) {} scale {} — scales {stops:?}, modes {modes:?}, saved {:?}",
                        m.name,
                        m.selector(),
                        m.mode.rule(),
                        m.scale,
                        GolemSettings::load().displays.get(&m.selector()),
                    );
                }
            }
            _ => warn!("display: unknown request {args:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percents(stops: &[f64]) -> Vec<u32> {
        stops.iter().map(|s| (s * 100.0).round() as u32).collect()
    }

    #[test]
    fn modes_read_from_the_compositor_and_write_as_rules() {
        let m = Mode::parse("1440x900@60.00Hz").unwrap();
        assert_eq!((m.w, m.h, m.hz), (1440, 900, 60.0));
        assert_eq!(m.rule(), "1440x900@60.00");
        assert_eq!(Mode::parse("3200x2000@165").unwrap().rule(), "3200x2000@165.00");
        assert_eq!(Mode::parse("1920x1080").unwrap().rule(), "1920x1080");
        for bad in ["", "preferred", "1440x", "x900", "0x900", "1440x900@nan", "1440x900@inf", "1440x900@-1", "a\"x9"] {
            assert!(Mode::parse(bad).is_none(), "{bad:?} is not a mode");
        }
    }

    #[test]
    fn every_offered_scale_divides_the_panel_into_whole_pixels() {
        for (w, h, current) in [(1440, 900, 1.0), (1920, 1080, 1.5), (3200, 2000, 1.6), (1366, 768, 1.0), (2560, 1440, 1.25)] {
            let stops = scale_stops(w, h, current);
            assert!(stops.iter().any(|s| (s - current).abs() < 1e-9), "{w}x{h}: the current scale is offered");
            for pair in stops.windows(2) {
                assert!(pair[1] / pair[0] >= OFFER_GAP, "{w}x{h}: {pair:?} crowd each other");
            }
            for s in stops {
                let (lw, lh) = (f64::from(w) / s, f64::from(h) / s);
                assert!((lw - lw.round()).abs() < 1e-6 && (lh - lh.round()).abs() < 1e-6, "{w}x{h} at {s}");
            }
        }
    }

    #[test]
    fn the_three_golem_panels_get_the_scales_expected() {
        // MacBook Air, ThinkPad, the dev box: round scales first, the
        // current one always there, nothing that hides the panel.
        assert_eq!(percents(&scale_stops(1440, 900, 1.0)), [75, 83, 90, 100, 113, 125, 150]);
        assert_eq!(percents(&scale_stops(1920, 1080, 1.5)), [75, 83, 100, 125, 150, 167, 200]);
        assert_eq!(percents(&scale_stops(3200, 2000, 1.6)), [80, 100, 125, 160, 200, 250]);
    }

    #[test]
    fn every_panel_always_has_a_scale_choice_and_a_way_home() {
        // Max (2026-10-02): "make sure all Golem installations have scale
        // options". Every width from 640 to 7680 px in eight shapes, plus
        // real panels: from 100 % follow every stop offered, and at each
        // one there are at least two choices, the current one, 100 % (the
        // way home) and nothing the compositor would refuse.
        let mut panels: Vec<(u32, u32)> = vec![
            (1366, 768), (1360, 768), (1280, 800), (1280, 1024), (1024, 768), (1024, 600), (800, 600),
            (1600, 900), (1680, 1050), (1920, 1200), (2256, 1504), (2736, 1824), (2880, 1800),
            (3000, 2000), (2560, 1080), (3440, 1440), (5120, 2880), (1080, 1920), (768, 1366),
        ];
        for w in (640..=7680).step_by(2) {
            for (a, b) in [(16, 9), (16, 10), (3, 2), (4, 3), (5, 4), (21, 9), (32, 9), (9, 16)] {
                panels.push((w, (f64::from(w) * f64::from(b) / f64::from(a)).round() as u32));
            }
        }
        let clean = |w: u32, h: u32, s: f64| {
            let (lw, lh) = (f64::from(w) / s, f64::from(h) / s);
            (lw - lw.round()).abs() < 1e-6 && (lh - lh.round()).abs() < 1e-6
        };
        for (w, h) in panels {
            let mut todo = vec![1.0_f64];
            let mut seen: Vec<f64> = Vec::new();
            while let Some(now) = todo.pop() {
                if seen.iter().any(|s| (s - now).abs() < 1e-9) {
                    continue;
                }
                seen.push(now);
                let stops = scale_stops(w, h, now);
                assert!(stops.len() >= 2, "{w}x{h} at {now}: no choice {stops:?}");
                assert!(stops.iter().any(|s| (s - now).abs() < 1e-9), "{w}x{h} at {now}: current missing");
                assert!(stops.iter().any(|s| (s - 1.0).abs() < 1e-9), "{w}x{h} at {now}: no way home {stops:?}");
                for &s in &stops {
                    assert!(clean(w, h, s), "{w}x{h}: {s} is not clean");
                    todo.push(s);
                }
            }
        }
    }

    #[test]
    fn a_panel_with_few_clean_scales_still_offers_a_choice() {
        // The ASUS X550LC: 1366×768, clean only at 50/67/100/200 %; 200 %
        // would be 683×384. 67 % (2049×1152) is the choice.
        assert_eq!(percents(&scale_stops(1366, 768, 1.0)), [67, 100]);
        // From 67 % the way back to 100 % is there too.
        assert_eq!(percents(&scale_stops(1366, 768, 80.0 / 120.0)), [67, 100]);
        // Every panel offers at least two scales.
        for (w, h) in [(1366, 768), (1440, 900), (1920, 1080), (1600, 900), (1280, 800), (3200, 2000), (1024, 600)] {
            assert!(scale_stops(w, h, 1.0).len() >= 2, "{w}x{h}: {:?}", scale_stops(w, h, 1.0));
        }
    }

    #[test]
    fn the_pointer_stays_where_it_is_on_the_glass() {
        // MacBook, 83 % -> 125 %: the pointer at pixel (700, 400).
        let old = 100.0 / 120.0;
        let cursor = (700.0 / old, 400.0 / old);
        let to = pointer_after(cursor, (0.0, 0.0), (1440, 900), old, 1.25).unwrap();
        assert!((to.0 * 1.25 - 700.0).abs() < 1e-6 && (to.1 * 1.25 - 400.0).abs() < 1e-6, "{to:?}");
        // A screen that does not start at the corner of the desk.
        let to = pointer_after((2100.0, 50.0), (2000.0, 0.0), (1920, 1080), 1.0, 1.5).unwrap();
        assert_eq!(to, (2000.0 + 100.0 / 1.5, 50.0 / 1.5));
        // On another screen: left alone.
        assert!(pointer_after((100.0, 100.0), (2000.0, 0.0), (1920, 1080), 1.0, 1.5).is_none());
        assert!(pointer_after((2000.0 + 1920.0, 10.0), (2000.0, 0.0), (1920, 1080), 1.0, 1.5).is_none());
    }

    #[test]
    fn a_rounded_scale_reads_back_as_the_clean_one() {
        assert_eq!(nearest_clean(1440, 900, 1.13), Some(1.125));
        assert_eq!(nearest_clean(1440, 900, 0.83), Some(100.0 / 120.0));
        assert_eq!(nearest_clean(3200, 2000, 1.6), Some(1.6));
        let json = serde_json::json!([{
            "name": "eDP-1", "description": "Apple Computer Inc Color LCD", "model": "Color LCD",
            "width": 1440, "height": 900, "refreshRate": 60.0, "scale": 1.13, "focused": true,
            "availableModes": ["1440x900@60.00Hz"],
        }]);
        let m = &parse_monitors(&json)[0];
        assert_eq!(m.scale, 1.125);
        assert_eq!(m.selector(), "desc:Apple Computer Inc Color LCD");
        assert_eq!(m.title(), "Built-in screen");
        let view = DisplayView::of(m);
        assert_eq!(view.looks_like(), (1280, 800));
        assert_eq!(view.sizes, [(1440, 900)]);
        assert_eq!(view.rates, [60.0]);
    }

    #[test]
    fn sizes_come_largest_first_and_rates_are_the_current_sizes() {
        let json = serde_json::json!([{
            "name": "DP-1", "description": "Dell U2720Q 123", "model": "U2720Q",
            "width": 2560, "height": 1440, "refreshRate": 59.95, "scale": 1.0, "focused": true,
            "availableModes": ["1920x1080@60.00Hz", "3840x2160@60.00Hz", "2560x1440@59.95Hz", "2560x1440@120.00Hz", "1920x1080@30.00Hz"],
        }]);
        let view = DisplayView::of(&parse_monitors(&json)[0]);
        assert_eq!(view.screen, "U2720Q");
        assert_eq!(view.sizes, [(3840, 2160), (2560, 1440), (1920, 1080)]);
        assert_eq!(view.rates, [59.95, 120.0]);
    }

    #[test]
    fn the_compositors_copy_holds_one_complete_rule_per_screen() {
        let mut store = GolemSettings::default();
        store.displays.insert(
            "desc:Apple Computer Inc Color LCD".to_owned(),
            DisplayChoice { mode: "1440x900@60.00".to_owned(), scale: 1.25, position: None },
        );
        let lua = store.lua();
        assert!(lua.lines().all(|l| l.starts_with("--") || l.starts_with("hl.monitor(")), "{lua}");
        assert!(lua.contains(
            r#"hl.monitor({ output = "desc:Apple Computer Inc Color LCD", mode = "1440x900@60.00", position = "auto", scale = 1.25 })"#
        ));
    }

    #[test]
    fn nothing_from_the_file_reaches_the_lua_unread() {
        let choice = |mode: &str, scale: f64, position: Option<&str>| DisplayChoice {
            mode: mode.to_owned(),
            scale,
            position: position.map(str::to_owned),
        };
        // A quote in a description stays inside its string.
        let rule = monitor_rule("desc:Evil \"}) os.exit() --\\", &choice("1440x900@60", 1.0, None)).unwrap();
        assert!(rule.starts_with(r#"hl.monitor({ output = "desc:Evil \"}) os.exit() --\\", mode = "#), "{rule}");
        // A mode, a scale or a place that does not read as one: no rule.
        assert!(monitor_rule("eDP-1", &choice("1440x900\" })", 1.0, None)).is_none());
        assert!(monitor_rule("eDP-1", &choice("1440x900", 0.0, None)).is_none());
        assert!(monitor_rule("eDP-1", &choice("1440x900", f64::NAN, None)).is_none());
        assert!(monitor_rule("eDP-1", &choice("1440x900", 1.0, Some("0x0\" })"))).is_none());
        assert!(monitor_rule("", &choice("1440x900", 1.0, None)).is_none());
        assert!(monitor_rule("eDP-1", &choice("1440x900", 1.0, Some("auto-left"))).is_some());
        assert!(monitor_rule("eDP-1", &choice("1440x900", 1.0, Some("-1920x0"))).is_some());
    }

    #[test]
    fn other_keys_of_the_settings_file_are_carried_through() {
        let text = r#"{ "dark": true, "gaps": { "inner": 4 }, "displays": { "eDP-1": { "mode": "1440x900@60.00", "scale": 1.5 } } }"#;
        let mut store: GolemSettings = serde_json::from_str(text).unwrap();
        assert_eq!(store.displays["eDP-1"].scale, 1.5);
        store.displays.clear();
        let back: serde_json::Value = serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert_eq!(back, serde_json::json!({ "dark": true, "gaps": { "inner": 4 } }));
    }
}

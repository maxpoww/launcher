//! Golem's colours: the PALETTE the whole desktop wears (Max, 2026-10-14:
//! "the pallete have to be everywhere, like omarchy does").
//!
//! A palette is sixteen colours in the base16 layout — `base00` (the
//! background) through `base07` (the brightest ink), then eight hues,
//! `base08` red to `base0F` brown — once for the dark look and once for the
//! light one, plus which of the hues is its accent. Golem's own ships inside
//! the daemon (`assets/palettes/`); the owner's own go in
//! `~/.config/golem/palettes/*.json`, same shape.
//!
//! The owner's choice lives under `colors` in `~/.config/golem/settings.json`
//! (Max, 2026-10-15: one page, "Colours" — the look, where the colours go,
//! then the palettes):
//!
//! - `palette` — which one. `dynamic` (the default) is a palette too: its
//!   colours are made from the wallpaper ([`dynamic_palette`]), and with it
//!   the shell keeps colouring itself from the screen, PLACE BY PLACE, as it
//!   always has (a box from the corner it opens in, the dock from the band
//!   under it) — the made palette never paints the shell. Any other palette
//!   paints the dock, the boxes and the window borders too.
//! - `apps` — on (the default): the programs (GTK, Qt, the terminal,
//!   Neovim, btop, Seam's own chrome) AND the webapps wear the palette. Off:
//!   they wear their makers' colours (the Stock palette: libadwaita's).
//! - `web` — off by default: websites in Seam stay as they are made. On:
//!   Seam recolours them in the palette (Dark Reader), in the dark look and
//!   in the light one. Webapps follow `apps`, not this (their hosts go to
//!   Seam in `colors.json`).
//!
//! The ACCENT is the palette's own (its file names the slot) — never chosen
//! apart from it (Max, 2026-10-15: "the acent colour should come from the
//! pallete, not apart"). A palette that wants another accent is another
//! palette.
//!
//! The dark/light choice (`dark`, the page's first switch) picks the
//! palette's variant.
//!
//! [`resolve`] makes the [`Theme`] — what is in use now — and [`publish`]
//! writes it out for everything outside the daemon, in
//! `~/.local/state/golem/theme/` (`colors.json` for anyone; `foot.ini`,
//! `hyprlock.conf`, `gtk4.css`, `qt.conf`, `nvim.lua` for the apps Golem's
//! home layer points there), then tells the running ones.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::display::GolemSettings;

/// One colour, sRGB bytes.
pub(crate) type Rgb = [u8; 3];

/// Golem's palette, the default.
pub(crate) const GOLEM: &str = "golem";

/// The palette made from the wallpaper (see [`dynamic_palette`]).
pub(crate) const DYNAMIC_ID: &str = "dynamic";

/// The makers' own colours (libadwaita's), what the programs wear when the
/// palette is kept off them.
const STOCK: &str = include_str!("../assets/palettes/stock.json");

/// The palettes inside the daemon: (id, file), Golem's first. The others are
/// base16 schemes from tinted-theming (MIT) whose hues sit in the standard
/// slots (red in base08… brown in base0F), as every role here assumes.
const BUILT_IN: &[(&str, &str)] = &[
    (GOLEM, include_str!("../assets/palettes/golem.json")),
];

/// THE CURATED BOX (Max, 2026-10-16; made in ~/Golem/work/palettes): the
/// palettes the Colours page offers, in its order — greys and neutrals, the
/// twelve colours of the artist's wheel, the combinations, the bold ones,
/// colour-blind safe and high contrast. Each carries its SOUL (the six
/// colours its thumbnail shows) and, where gold would not read on paper, an
/// accent of its own for the light look. (Dynamic is made, not listed here.)
const CURATED: &str = include_str!("../assets/palettes/golem-palettes.json");

/// The owner's colour choice, as saved in `settings.json` (Max, 2026-10-16:
/// the Colours page). Three sliders say how much of the palette goes where —
/// `apps` (the programs' and webapps' grounds), `web` (the pages in Seam),
/// `icons` (the dock's icons) — each 0 = OFF (their own colours), 1..100 a
/// walk from neutral grey through the palette as made to its colour at full
/// strength, in even visible steps ([`tint`]). `None` = the palette as made
/// (what a fresh setting is: the knob never moves by itself). Beside each, a
/// Dark/Light of its own: `None` follows the system look until clicked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ColorChoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_tint: Option<u8>,
    /// (Always written: null — as made — must not read back as the default, off.)
    #[serde(default = "web_off")]
    pub web_tint: Option<u8>,
    #[serde(default)]
    pub icons_tint: u8,
    /// THE SHELL (the dock, the boxes; Max, 2026-10-17): None = as made
    /// (what it was before this slider), 0 = off (it reads the screen again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_tint: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_dark: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_dark: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_dark: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons_dark: Option<bool>,
    /// The apps that keep their OWN colours (desktop ids; Max, 2026-10-16:
    /// the "+" beside Apps, one switch each).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<String>,
    /// LIGHTING (Max, 2026-10-18): how much light the whole system has, 0..100;
    /// None = 50, as designed. Moves every ground within its look's band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<u8>,
    /// AUTO LIGHTING (Max, 2026-10-18): the lighting follows the sun —
    /// day as designed, night down to 20 — changed only in front of the
    /// owner (`lighting.rs`). None = off: the slider is the owner's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting_auto: Option<bool>,
    /// "Include the background": the lighting darkens the wallpaper too
    /// (a black wash under the desktop's icons). None = off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting_veil: Option<bool>,
    /// COLOURS ON THE WALLPAPERS (Max, 2026-10-18: "a slider at the top of
    /// the wallpaper page, that says 'Colours'"): how far the pictures are
    /// moved toward the palette, 0..100. None / 0 = the pictures as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallpaper_tint: Option<u8>,
    // THE COLOURS MODEL (Max, 2026-10-20; docs/colours-model.md): each surface
    // its Lighting within its look (None = 50, as designed) and its Saturation
    // (None = 50, as made); the wallpaper its own look; Auto and Ambient per
    // tab; each app's own look. `lighting`/`lighting_auto`/`lighting_veil`
    // above are the old general ones, carried into these once (`settle_lines`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_light: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_light: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons_light: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_light: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_light: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_sat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_sat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons_sat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_sat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_sat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_dark: Option<bool>,
    /// AUTO of the Colours tab: Golem's defaults for the four surfaces, Ambient
    /// with them (None = off; a fresh install is set on by `settle_lines`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_auto: Option<bool>,
    /// AMBIENT of the Colours tab: the light of the four surfaces follows the sun.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ambient: Option<bool>,
    /// An app's own look, by desktop id: "dark" or "light" (absent: the Apps').
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub app_look: std::collections::BTreeMap<String, String>,
    /// Set once the general Lighting was carried into the surfaces' own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<bool>,
    /// THE TRANSPARENCY, a slider each (Max, 2026-10-27): 0 solid … 100
    /// nearly invisible; None = as made. The Shell's glass, the apps' and
    /// Seam's windows, and the dock's icons (the Icons section's "Dock icons").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_clear: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_clear: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_clear: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock_clear: Option<u8>,
    /// Before 2026-10-16 the apps and the web were switches; read once.
    #[serde(default, skip_serializing)]
    pub apps: Option<bool>,
    #[serde(default, skip_serializing, alias = "dark_websites")]
    pub web: Option<bool>,
    /// Before 2026-10-15 the screen's colours were a switch of their own
    /// beside the palettes; read once, never written again.
    #[serde(default, skip_serializing)]
    pub dynamic: Option<bool>,
}

fn web_off() -> Option<u8> {
    Some(0)
}

// ------------------------------------------------------- the transparency

/// Where the Shell's slider stands as made: its glass as it is today.
pub(crate) const SHELL_CLEAR_MADE: u8 = 35;

/// A Transparency slider's as-made place: the Shell's glass, else solid.
pub(crate) fn clear_made(s: Surface) -> u8 {
    if s == Surface::Shell {
        SHELL_CLEAR_MADE
    } else {
        0
    }
}

impl ColorChoice {
    /// A section's Transparency (0 solid … 100 nearly invisible); the Icons
    /// section's is the dock's icons'.
    pub(crate) fn clear_of(&self, s: Surface) -> u8 {
        // (Auto is the tab's Golem defaults: as made, as its other sliders.)
        if self.auto_of(s) {
            return clear_made(s);
        }
        let v = match s {
            Surface::Shell => self.shell_clear,
            Surface::Apps => self.apps_clear,
            Surface::Web => self.web_clear,
            Surface::Icons => self.dock_clear,
            Surface::Wall => None,
        };
        v.unwrap_or_else(|| clear_made(s)).min(100)
    }
}

/// A Transparency as the opacity of what it covers whole (a window, the
/// dock's icons): 1 solid, 0.05 at the far end — never 0: a window no one
/// sees still takes the clicks.
pub(crate) fn clear_alpha(v: u8) -> f32 {
    1.0 - 0.95 * f32::from(v.min(100)) / 100.0
}

/// Every section's Transparency, as the Colours box opened.
static CLEAR_AT_OPEN: std::sync::Mutex<Option<[u8; 4]>> = std::sync::Mutex::new(None);

fn clears(c: &ColorChoice) -> [u8; 4] {
    [Surface::Shell, Surface::Apps, Surface::Web, Surface::Icons].map(|s| c.clear_of(s))
}

/// The Colours box opened: what it costs is said as it closes, only if then
/// something is MORE transparent than now (moved back, or toward Solid, it is
/// not worth a word).
pub(crate) fn clear_remember() {
    if let Ok(mut g) = CLEAR_AT_OPEN.lock() {
        *g = Some(clears(&choice()));
    }
}

/// The Colours box closed: whether a Transparency rose in this visit, past
/// as made (what Golem is made with costs no word).
pub(crate) fn clear_rose() -> bool {
    let before = CLEAR_AT_OPEN.lock().ok().and_then(|mut g| g.take());
    before.is_some_and(|b| rose(b, clears(&choice())))
}

fn rose(was: [u8; 4], now: [u8; 4]) -> bool {
    let made = [Surface::Shell, Surface::Apps, Surface::Web, Surface::Icons].map(clear_made);
    (0..4).any(|i| now[i] > was[i] && now[i] > made[i])
}

/// The windows' rules last told the compositor: told again only when they
/// change (`colors_apply` runs at every step of the light).
static RULES_TOLD: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The compositor's rules for the windows: every window wears the Apps'
/// opacity, Seam's own windows the Web's (a webapp is an app: its class is
/// `webapp-…`), and a window in full screen is always solid (the third
/// value). The later rule wins for Seam, so it is always there while the
/// first is. Off (both solid): both rules are turned off.
pub(crate) fn clear_rules_lua(c: &ColorChoice) -> String {
    let (apps, web) = (clear_alpha(c.clear_of(Surface::Apps)), clear_alpha(c.clear_of(Surface::Web)));
    let any = apps < 1.0 || web < 1.0;
    // (In a pcall of their own: settings.lua runs as one, and a rule the
    // compositor refused must not take the screens and keys after it along.)
    format!(
        "pcall(function() hl.window_rule({{ name = \"golem-clear-apps\", match = {{ class = \".*\" }}, opacity = \"{apps:.2} {apps:.2} 1.0\", enabled = {} }}) \
         hl.window_rule({{ name = \"golem-clear-web\", match = {{ class = \"^(seam|firefox)$\" }}, opacity = \"{web:.2} {web:.2} 1.0\", enabled = {any} }}) end)",
        apps < 1.0
    )
}

impl Default for ColorChoice {
    fn default() -> Self {
        Self { palette: None, apps_tint: None, web_tint: Some(0), icons_tint: 0, shell_tint: None, shell_dark: None, apps_dark: None, web_dark: None, icons_dark: None, skip: Vec::new(), lighting: None, lighting_auto: None, lighting_veil: None, wallpaper_tint: None, shell_light: None, apps_light: None, icons_light: None, web_light: None, wall_light: None, shell_sat: None, apps_sat: None, icons_sat: None, web_sat: None, wall_sat: None, wall_dark: None, auto: None, wall_auto: None, ambient: None, wall_ambient: None, app_look: Default::default(), lines: None, shell_clear: None, apps_clear: None, web_clear: None, dock_clear: None, apps: None, web: None, dynamic: None }
    }
}

impl ColorChoice {
    /// The palette asked for. The old switch on meant the shell took the
    /// screen's colours: that is the Dynamic palette now.
    pub(crate) fn palette_id(&self) -> &str {
        match (self.palette.as_deref(), self.dynamic) {
            (_, Some(true)) | (None, None) => DYNAMIC_ID,
            (Some(p), _) => p,
            (None, Some(false)) => GOLEM,
        }
    }
    /// The Apps slider, the old switch carried over (off → 0).
    pub(crate) fn apps_level(&self) -> Option<u8> {
        if self.apps == Some(false) && self.apps_tint.is_none() { Some(0) } else { self.apps_tint }
    }
    /// The Web slider, the old switch carried over (on → as made).
    pub(crate) fn web_level(&self) -> Option<u8> {
        if self.web == Some(true) && self.web_tint == Some(0) { None } else { self.web_tint }
    }
    /// Written back without the old switches, their meaning moved to the sliders.
    pub(crate) fn settled(mut self) -> Self {
        self.apps_tint = self.apps_level();
        self.web_tint = self.web_level();
        self.apps = None;
        self.web = None;
        self
    }

    /// Whether a surface's tab is in Auto (Golem's defaults).
    pub(crate) fn auto_of(&self, s: Surface) -> bool {
        match s {
            Surface::Wall => self.wall_auto,
            _ => self.auto,
        }
        .unwrap_or(false)
    }
    /// THE APPS' EXCEPTIONS IN FORCE — the apps kept in their own colours
    /// and the apps' own looks: none under Auto (Max, 2026-10-21: "auto is
    /// auto, there is no per apps when auto"). Kept, for Auto turned off.
    pub(crate) fn skip_in_force(&self) -> &[String] {
        if self.auto_of(Surface::Apps) {
            &[]
        } else {
            &self.skip
        }
    }
    pub(crate) fn looks_in_force(&self) -> &std::collections::BTreeMap<String, String> {
        static NONE: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        if self.auto_of(Surface::Apps) {
            &NONE
        } else {
            &self.app_look
        }
    }
    /// Whether a surface's light follows the sun (Auto has it on too).
    pub(crate) fn ambient_of(&self, s: Surface) -> bool {
        self.auto_of(s)
            || match s {
                Surface::Wall => self.wall_ambient,
                _ => self.ambient,
            }
            .unwrap_or(false)
    }

    /// What a surface wears: its own levels; under Ambient the sun's look and
    /// light (`sun`); under Auto Golem's defaults besides (the palette as made,
    /// icons untinted, the wallpaper at Palette 0% — a picture as it is — and
    /// saturation as made). `system_dark` stands in for a look never chosen.
    pub(crate) fn levels(&self, s: Surface, sun: Option<(bool, u8)>, system_dark: bool) -> Levels {
        let auto = self.auto_of(s);
        let (own_dark, own_light, own_tint, own_sat) = match s {
            Surface::Shell => (self.shell_dark, self.shell_light, self.shell_tint, self.shell_sat),
            Surface::Apps => (self.apps_dark, self.apps_light, self.apps_level(), self.apps_sat),
            Surface::Icons => (self.icons_dark, self.icons_light, Some(self.icons_tint), self.icons_sat),
            Surface::Web => (self.web_dark, self.web_light, self.web_level(), self.web_sat),
            Surface::Wall => (self.wall_dark, self.wall_light, Some(self.wallpaper_tint.unwrap_or(0)), self.wall_sat),
        };
        let (dark, light) = match (self.ambient_of(s), sun) {
            (true, Some((d, l))) => (d, l.min(100)),
            _ => (own_dark.unwrap_or(system_dark), own_light.unwrap_or(50).min(100)),
        };
        let (tint, sat) = if auto {
            let tint = match s {
                // (0% for every wallpaper: Max, 2026-10-21, "set BG auto to:
                // hambient on, palette 0% saturation as made". It was 100%
                // for the palette's own, which grades the box's pictures whole.)
                Surface::Icons | Surface::Wall => Some(0),
                _ => None,
            };
            (tint, 50)
        } else {
            (own_tint, own_sat.unwrap_or(50).min(100))
        };
        Levels { dark, light, tint, sat }
    }

    /// [`levels`] with what is true now: the sun's line while a fade shows it,
    /// and the shell's look.
    pub(crate) fn levels_now(&self, s: Surface) -> Levels {
        self.levels(s, crate::lighting::ambient_line(), crate::look::dark())
    }
}

/// A surface the colours dress; each has its own three (docs/colours-model.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Surface {
    Shell,
    Apps,
    Icons,
    Web,
    Wall,
}

/// What a surface wears now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Levels {
    pub dark: bool,
    /// Its Lighting within the look: 0..100, 50 as designed.
    pub light: u8,
    /// Its Palette slider: None = as made, Some(0) = none (the makers' own
    /// colours; a picture as it is), else how far toward full colour.
    pub tint: Option<u8>,
    /// Its Saturation: 0 grey, 50 as made, 100 vivid.
    pub sat: u8,
}

/// ONE LINE for a surface's Lighting (the box's slider): 0..45 the Dark look,
/// 55..100 the Light look, 46..54 the gap the knob leaps over; 50 of each half
/// is as designed.
#[allow(dead_code)]
pub(crate) fn line_of(dark: bool, light: u8) -> u8 {
    let w = f32::from(light.min(100)) / 100.0 * 45.0;
    if dark { w.round() as u8 } else { 55 + w.round() as u8 }
}
/// Ambient or Auto turned off: each surface's sliders take the look and the
/// light the sun was giving (`sun`), so nothing jumps.
fn freeze(c: &mut ColorChoice, surfaces: &[Surface], sun: Option<(bool, u8)>) {
    let Some((dark, light)) = sun else { return };
    for s in surfaces {
        let (d, l) = match s {
            Surface::Shell => (&mut c.shell_dark, &mut c.shell_light),
            Surface::Apps => (&mut c.apps_dark, &mut c.apps_light),
            Surface::Icons => (&mut c.icons_dark, &mut c.icons_light),
            Surface::Web => (&mut c.web_dark, &mut c.web_light),
            Surface::Wall => (&mut c.wall_dark, &mut c.wall_light),
        };
        *d = Some(dark);
        *l = Some(light);
    }
}
pub(crate) fn line_to(v: u8) -> (bool, u8) {
    let v = v.min(100);
    if v < 50 { (true, (f32::from(v.min(45)) / 45.0 * 100.0).round() as u8) } else { (false, (f32::from(v.max(55) - 55) / 45.0 * 100.0).round() as u8) }
}

/// A palette file.
#[derive(Debug, Clone, Deserialize)]
struct PaletteFile {
    name: String,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    accent: Option<String>,
    /// The accent in the light look, when it is another (gold on paper).
    #[serde(default)]
    accent_light: Option<String>,
    /// The six colours of its thumbnail.
    #[serde(default)]
    soul: Vec<String>,
    dark: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    light: Option<std::collections::BTreeMap<String, String>>,
}

/// A palette, read and checked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Palette {
    pub id: String,
    pub name: String,
    pub dark: [Rgb; 16],
    pub light: [Rgb; 16],
    /// The slot of its accent (8..=15) in the dark look…
    pub accent: usize,
    /// …and in the light one (the same unless the file names another).
    pub accent_light: usize,
    /// The six colours of its thumbnail (empty for an older file).
    pub soul: Vec<Rgb>,
    pub family: String,
}

/// What is in use now: one variant of one palette, with the accent picked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Theme {
    pub id: String,
    pub name: String,
    pub dark: bool,
    /// The Dynamic palette: the shell colours itself from the screen.
    pub dynamic: bool,
    /// The programs and webapps wear it (else the Stock colours).
    pub apps: bool,
    /// Websites in Seam wear it.
    pub web: bool,
    pub c: [Rgb; 16],
    pub accent_slot: usize,
    /// The sliders and their looks (see [`ColorChoice`]); the looks resolved.
    pub apps_tint: Option<u8>,
    pub web_tint: Option<u8>,
    pub icons_tint: u8,
    pub apps_dark: bool,
    pub web_dark: bool,
    pub icons_dark: bool,
    /// Each surface's Lighting within its look and its Saturation (see [`Levels`]).
    pub apps_light: u8,
    pub apps_sat: u8,
    pub web_light: u8,
    pub web_sat: u8,
    pub icons_sat: u8,
    /// Both looks as the programs wear them (tinted, or the makers' own with
    /// the palette's accent): what writes both at once (foot, GTK 4) reads.
    pub pair: Option<([Rgb; 16], usize, [Rgb; 16], usize)>,
    /// The palette in EACH look as the programs would wear it, whatever look
    /// the apps keep (`pair` gives both halves the apps' look when it is not
    /// the shell's): what a program or a webapp pinned to Dark or Light
    /// wears. (Pinned from `pair`, a Dark app wore the light colours whenever
    /// the apps were Light under a Dark shell — the pass of 2026-10-27.)
    pub looks: Option<([Rgb; 16], usize, [Rgb; 16], usize)>,
}

impl Theme {
    pub(crate) fn bg(&self) -> Rgb {
        self.c[0x00]
    }
    pub(crate) fn surface(&self) -> Rgb {
        self.c[0x01]
    }
    pub(crate) fn raised(&self) -> Rgb {
        self.c[0x02]
    }
    pub(crate) fn border(&self) -> Rgb {
        self.c[0x03]
    }
    pub(crate) fn ink_dim(&self) -> Rgb {
        self.c[0x04]
    }
    pub(crate) fn ink(&self) -> Rgb {
        self.c[0x05]
    }
    /// QUIET WORDS: the "comment" grey of the programs that follow base16 —
    /// Neovim's comments and line numbers, the terminal's bright black, btop's
    /// inactive, the Text Editor's comments and line numbers. They take
    /// base03 for it, which in Golem's palettes is a GROUND (the fourth step
    /// the Lighting moves): invisible in nearly every palette (the legibility
    /// audit, 2026-10-23). The dimmest grey of its hue that reads 4.5:1 on
    /// base00 and base01 ([`as_text`]).
    pub(crate) fn quiet(&self) -> Rgb {
        quiet(&self.c)
    }
    pub(crate) fn accent(&self) -> Rgb {
        self.c[self.accent_slot]
    }
    /// A colour read AS TEXT on these grounds (a link, the accent in a
    /// label, an error's red): see [`as_text`].
    pub(crate) fn as_text(&self, x: Rgb) -> Rgb {
        as_text(&self.c, x)
    }
    /// The accent as text; fills (buttons, badges, a selection) keep the
    /// accent itself — libadwaita's accent_color beside accent_bg_color.
    pub(crate) fn accent_text(&self) -> Rgb {
        self.as_text(self.accent())
    }
    /// An editor's selection: the accent a third over the ground, less of
    /// it where the text would not read 4.5:1 on it (Lighting 100 in the
    /// dark took it to 3.7, 2026-10-20).
    pub(crate) fn selection(&self) -> Rgb {
        let mix = |k: f32| -> Rgb {
            std::array::from_fn(|i| (f32::from(self.accent()[i]) + (f32::from(self.c[0][i]) - f32::from(self.accent()[i])) * k).round() as u8)
        };
        let mut k = 0.65;
        while k < 1.0 && contrast(rel_lum(self.ink()), rel_lum(mix(k))) < 4.5 {
            k += 0.02;
        }
        mix(k.min(1.0))
    }
    /// The ink that reads ON the accent: the palette's darkest or its
    /// brightest, whichever stands further from it.
    pub(crate) fn accent_ink(&self) -> Rgb {
        // (A mid-tone accent neither end reads on — the sweep: Braun's
        // #d14118 took 4.2:1 — gets pure black or white.)
        ink_on_fill(&self.c, self.accent())
    }
}

/// `#rrggbb` (or `rrggbb`) → bytes.
pub(crate) fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

pub(crate) fn hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// `base0D` → 13.
fn slot(name: &str) -> Option<usize> {
    let n = name.trim().to_ascii_lowercase();
    let d = n.strip_prefix("base")?;
    if d.len() != 2 {
        return None;
    }
    usize::from_str_radix(d, 16).ok().filter(|s| *s < 16)
}

pub(crate) fn slot_name(s: usize) -> String {
    format!("base{s:02X}")
}

fn sixteen(map: &std::collections::BTreeMap<String, String>) -> Option<[Rgb; 16]> {
    let mut out = [[0u8; 3]; 16];
    let mut seen = [false; 16];
    for (k, v) in map {
        let s = slot(k)?;
        out[s] = parse_hex(v)?;
        seen[s] = true;
    }
    seen.iter().all(|s| *s).then_some(out)
}

/// A light variant for a palette that brings none: its greys turned over
/// (base00 ↔ base07…), its hues as they are.
fn turned_over(dark: &[Rgb; 16]) -> [Rgb; 16] {
    let mut out = *dark;
    for i in 0..8 {
        out[i] = dark[7 - i];
    }
    out
}

fn parse(id: &str, text: &str) -> Option<Palette> {
    let file: PaletteFile = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            warn!("palette {id}: not readable: {e}");
            return None;
        }
    };
    let Some(dark) = sixteen(&file.dark) else {
        warn!("palette {id}: its dark variant needs base00..base0F, each #rrggbb");
        return None;
    };
    let light = match &file.light {
        Some(l) => match sixteen(l) {
            Some(l) => l,
            None => {
                warn!("palette {id}: its light variant needs base00..base0F, each #rrggbb");
                return None;
            }
        },
        None => turned_over(&dark),
    };
    let accent = file.accent.as_deref().and_then(slot).filter(|s| *s >= 8).unwrap_or(0x0D);
    let accent_light = file.accent_light.as_deref().and_then(slot).filter(|s| *s >= 8).unwrap_or(accent);
    let soul = file.soul.iter().filter_map(|h| parse_hex(h)).collect();
    Some(Palette { id: id.to_owned(), name: file.name, dark, light, accent, accent_light, soul, family: file.family.unwrap_or_default() })
}

/// Keep one of the owner's palettes in their directory (`<id>.json`).
pub(crate) fn save_own(p: &Palette) -> std::io::Result<()> {
    let map = |c: &[Rgb; 16]| (0..16).map(|i| (slot_name(i), format!("#{}", hex(c[i])))).collect::<std::collections::BTreeMap<_, _>>();
    let file = serde_json::json!({
        "name": p.name,
        "family": p.family,
        "accent": slot_name(p.accent),
        "accent_light": slot_name(p.accent_light),
        "soul": p.soul.iter().map(|c| format!("#{}", hex(*c))).collect::<Vec<_>>(),
        "dark": map(&p.dark),
        "light": map(&p.light),
    });
    let dir = own_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(&file).unwrap_or_default() + "\n";
    // (Written whole or not at all, and a failure said: Save picks it next.)
    let path = dir.join(format!("{}.json", p.id));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

/// The apps' look told the system (dconf) once a change and in order: a
/// drag across the Lighting's gap is many steps, and threads racing each
/// other could leave the system on the other side of the row. Each teller
/// waits its turn and tells the look asked for LAST.
fn tell_apps_look(dark: bool) {
    static TOLD: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    if TOLD.lock().is_ok_and(|t| *t == Some(dark)) {
        return;
    }
    let _ = std::thread::Builder::new().name("apps-look".into()).spawn(|| {
        let Ok(mut told) = TOLD.lock() else { return };
        let now = choice().levels_now(Surface::Apps).dark;
        if *told != Some(now) {
            crate::controls::assert_dark(now);
            *told = Some(now);
        }
    });
}

/// The owner's own palettes' directory (none under test: the tests must not
/// depend on, nor write, what the owner made).
fn own_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("golem-test-no-own-palettes");
    }
    config_home().join("golem").join("palettes")
}

fn config_home() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
}

/// Where the theme is written for the apps.
pub(crate) fn theme_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state"))
        .join("golem")
        .join("theme")
}

/// The curated box, read once.
fn curated() -> &'static [Palette] {
    static ONCE: std::sync::OnceLock<Vec<Palette>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let files: Vec<serde_json::Value> = serde_json::from_str(CURATED).unwrap_or_default();
        files
            .iter()
            .filter_map(|v| {
                let id = v.get("id")?.as_str()?.to_owned();
                parse(&id, &v.to_string())
            })
            .collect()
    })
}

/// The palettes that ship (Golem's own and the curated box), for the
/// checks every one of them must pass.
#[cfg(test)]
pub(crate) fn shipped() -> Vec<Palette> {
    let mut out: Vec<Palette> = BUILT_IN.iter().filter_map(|(id, text)| parse(id, text)).collect();
    out.extend(curated().iter().cloned());
    out
}

/// Every palette there is: the curated box in its order, then the owner's by name.
pub(crate) fn all() -> Vec<Palette> {
    let mut out: Vec<Palette> = curated().to_vec();
    let mut own: Vec<Palette> = std::fs::read_dir(own_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                return None;
            }
            let id = p.file_stem()?.to_str()?.to_owned();
            if BUILT_IN.iter().any(|(b, _)| *b == id) || curated().iter().any(|c| c.id == id) {
                return None;
            }
            parse(&id, &std::fs::read_to_string(&p).ok()?)
        })
        .collect();
    own.sort_by_key(|p| p.name.to_lowercase());
    out.extend(own);
    out
}

/// The choice as saved now.
pub(crate) fn choice() -> ColorChoice {
    GolemSettings::load().colors.unwrap_or_default()
}

/// A palette's SWATCH in a look (what its card and its wallpaper are drawn
/// with): the ground of that look first, then the palette's soul — its own
/// six, else the bright half of its sixteen.
pub(crate) fn swatch(p: &Palette, dark: bool) -> Vec<Rgb> {
    let mut out = vec![if dark { p.dark[0] } else { p.light[0] }];
    out.extend(if p.soul.is_empty() { p.dark[8..16].to_vec() } else { p.soul.clone() });
    out
}

/// The swatch of the palette in use, in the shell's look. `None` for
/// Dynamic (it has no colours of its own: they are made from the wallpaper)
/// and for a palette that is not there.
pub(crate) fn swatch_in_use() -> Option<Vec<Rgb>> {
    let c = choice();
    let id = c.palette_id();
    if id == DYNAMIC_ID {
        return None;
    }
    palette_of(id).map(|p| swatch(&p, c.levels_now(Surface::Wall).dark))
}

/// The general Dark toggle is gone (Max, 2026-10-18): each row — apps, web,
/// icons, shell — has its own Dark/Light. A row that used to follow the
/// general one takes, once, the look it was showing, so an update changes
/// nothing anyone sees.
pub(crate) fn settle_looks() {
    let c = choice();
    if c.apps_dark.is_some() && c.web_dark.is_some() && c.icons_dark.is_some() && c.shell_dark.is_some() {
        return;
    }
    let was = crate::controls::dark_now();
    GolemSettings::change(|s| {
        let mut c = s.colors.clone().unwrap_or_default();
        for look in [&mut c.apps_dark, &mut c.web_dark, &mut c.icons_dark, &mut c.shell_dark] {
            look.get_or_insert(was);
        }
        s.colors = Some(c);
        s.store();
    });
    info!("palette: each row keeps the look it showed ({})", if was { "dark" } else { "light" });
}

/// THE LINES (2026-10-20): once, the general Lighting becomes each surface's
/// own, Auto lighting becomes Ambient and "Include the background" the
/// wallpaper's Ambient; a machine with colours of its own stays manual, a
/// fresh one starts in Auto (Golem's defaults, the light with the sun).
pub(crate) fn settle_lines() {
    let had = GolemSettings::load().colors;
    if had.as_ref().is_some_and(|c| c.lines == Some(true)) {
        return;
    }
    let fresh = had.is_none();
    GolemSettings::change(|s| {
        let mut c = s.colors.clone().unwrap_or_default();
        if fresh {
            c.auto = Some(true);
            c.wall_auto = Some(true);
        } else {
            let l = c.lighting;
            for own in [&mut c.shell_light, &mut c.apps_light, &mut c.icons_light, &mut c.web_light] {
                if own.is_none() {
                    *own = l;
                }
            }
            c.ambient = c.lighting_auto.filter(|a| *a);
            c.wall_ambient = (c.lighting_auto == Some(true) && c.lighting_veil == Some(true)).then_some(true);
            c.wall_dark = c.wall_dark.or(c.shell_dark);
            c.auto = Some(false);
            c.wall_auto = Some(false);
        }
        c.lines = Some(true);
        s.colors = Some(c);
        s.store();
    });
    info!("palette: the lighting is each surface's own ({})", if fresh { "a fresh start: Auto" } else { "carried over" });
}

/// The theme a choice gives in a look.
/// The palette being MADE in the editor, worn while it is made (Max: "the
/// desktop shows it as you go"); never saved as the choice.
static PREVIEW: std::sync::Mutex<Option<Palette>> = std::sync::Mutex::new(None);
pub(crate) const DRAFT_ID: &str = "golem-draft";

pub(crate) fn set_preview(p: Option<Palette>) {
    if let Ok(mut g) = PREVIEW.lock() {
        *g = p;
    }
}

pub(crate) fn previewing() -> bool {
    PREVIEW.lock().is_ok_and(|g| g.is_some())
}

fn preview() -> Option<Palette> {
    PREVIEW.lock().ok().and_then(|g| g.clone())
}

pub(crate) fn resolve(choice: &ColorChoice, dark: bool) -> Theme {
    let want = choice.palette_id();
    let p = if want == DRAFT_ID {
        preview()
    } else if want == DYNAMIC_ID {
        dynamic_palette()
    } else {
        all().into_iter().find(|p| p.id == want)
    }
    .unwrap_or_else(golem);
    // Each surface through its levels: Auto's defaults and Ambient's sun apply
    // here (the programs and the web through the held look, if one).
    let apps = choice.levels(Surface::Apps, crate::lighting::ambient_line(), dark);
    let web = choice.levels(Surface::Web, crate::lighting::ambient_line(), dark);
    let icons = choice.levels(Surface::Icons, crate::lighting::ambient_line(), dark);
    let apps_tint = apps.tint;
    let web_tint = web.tint;
    // (The shell's own hues kept readable too: Fluor's and Golem's light
    // accents stood at 2.5:1 on their surface.)
    let (mut c, acc) = if dark { (p.dark, p.accent) } else { (p.light, p.accent_light) };
    readable(&mut c, dark, acc);
    Theme {
        dynamic: want == DYNAMIC_ID,
        id: if want == DYNAMIC_ID { DYNAMIC_ID.to_owned() } else { p.id },
        name: p.name,
        dark,
        apps: apps_tint != Some(0),
        web: web_tint != Some(0),
        c,
        accent_slot: acc,
        apps_tint,
        web_tint,
        icons_tint: icons.tint.unwrap_or(0),
        apps_dark: apps.dark,
        web_dark: web.dark,
        icons_dark: icons.dark,
        apps_light: apps.light,
        apps_sat: apps.sat,
        web_light: web.light,
        web_sat: web.sat,
        icons_sat: icons.sat,
        pair: None,
        looks: None,
    }
}

fn golem() -> Palette {
    parse(GOLEM, BUILT_IN[0].1).expect("Golem's palette parses")
}

/// The makers' colours, in the same look as `t`: what the programs wear
/// when the palette is kept off them.
#[cfg(test)]
pub(crate) fn stock(t: &Theme) -> Theme {
    let p = parse("stock", STOCK).expect("the Stock palette parses");
    Theme { id: p.id, name: p.name, c: if t.dark { p.dark } else { p.light }, accent_slot: p.accent, pair: None, ..t.clone() }
}

// ------------------------------------------------------------ dynamic

/// THE DYNAMIC PALETTE (Max, 2026-10-15: "para mi color dinamico cuenta
/// como paleta"): sixteen colours made from the wallpaper, for the programs
/// and the web — the shell keeps reading the screen place by place.
///
/// - the greys (base00..07) are the look's usual steps, tinted faintly
///   toward the picture's own hue;
/// - the eight hues sit at their standard places (red in base08…), each
///   leaned a little toward the picture's colour, so they belong to it;
/// - the ACCENT is the picture's most present vivid colour, put in the hue
///   slot nearest it, at a lightness that reads in each look. A grey
///   picture keeps a blue accent.
///
/// Made once per picture (and kept while it stays the same file); the first
/// reading of a new one happens on a thread, and Golem's palette stands in
/// meanwhile ([`App::colors_apply`] comes back when it is ready).
pub(crate) fn dynamic_palette() -> Option<Palette> {
    let path = GolemSettings::load().wallpaper?;
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let cache = DYNAMIC_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match &*cache {
        Some((p, t, made)) if *p == path && *t == stamp => return made.clone(),
        _ => {}
    }
    if DYNAMIC_BUSY.swap(true, Ordering::AcqRel) {
        return None;
    }
    drop(cache);
    let spawned = std::thread::Builder::new().name("palette-dynamic".into()).spawn(move || {
        let made = wallpaper_pixels(&path).map(|px| from_pixels(&px));
        info!("palette: the dynamic palette from {path}: {}", if made.is_some() { "made" } else { "not readable, Golem's stands in" });
        *DYNAMIC_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, stamp, made));
        DYNAMIC_BUSY.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        DYNAMIC_BUSY.store(false, Ordering::Release);
    }
    None
}

/// Whether the dynamic palette is being made now.
pub(crate) fn dynamic_pending() -> bool {
    DYNAMIC_BUSY.load(Ordering::Acquire)
}

type DynamicCache = Option<(String, std::time::SystemTime, Option<Palette>)>;
static DYNAMIC_CACHE: std::sync::Mutex<DynamicCache> = std::sync::Mutex::new(None);
static DYNAMIC_BUSY: AtomicBool = AtomicBool::new(false);

/// The wallpaper, small, as sRGB bytes (a video: its first frame, the still
/// the wallpaper already keeps).
fn wallpaper_pixels(path: &str) -> Option<Vec<[u8; 3]>> {
    let still = if crate::controls::wallpaper_moves(path) && !path.to_lowercase().ends_with(".gif") {
        crate::controls::poster(path)?
    } else {
        path.to_owned()
    };
    let img = image::open(&still).ok()?.thumbnail(64, 64).to_rgb8();
    Some(img.pixels().map(|p| p.0).collect())
}

fn hsl(c: Rgb) -> (f32, f32, f32) {
    let f = |v: u8| f32::from(v) / 255.0;
    crate::options::rgb_to_hsl(f(c[0]), f(c[1]), f(c[2]))
}

fn rgb(h: f32, s: f32, l: f32) -> Rgb {
    let (r, g, b) = crate::options::hsl_to_rgb(h.rem_euclid(1.0), s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(r), q(g), q(b)]
}

/// The hues' standard places (fractions of the circle), base08..base0F.
const HUES: [f32; 8] = [0.0 / 360.0, 26.0 / 360.0, 46.0 / 360.0, 120.0 / 360.0, 176.0 / 360.0, 214.0 / 360.0, 285.0 / 360.0, 24.0 / 360.0];

/// The palette a picture gives (see [`dynamic_palette`]).
pub(crate) fn from_pixels(px: &[[u8; 3]]) -> Palette {
    // The picture's hue (all of it, weighted by colour) and its most
    // present vivid colour: a 24-bin hue histogram of the pixels with
    // colour enough, each weighted by saturation.
    const BINS: usize = 24;
    let mut bins = [(0.0f32, 0.0f32); BINS]; // weight, Σ w·s
    let mut vec = (0.0f32, 0.0f32);
    let mut sincos = [(0.0f32, 0.0f32); BINS];
    for c in px {
        let (h, s, l) = hsl(*c);
        let w = s * (1.0 - (2.0 * l - 1.0).abs());
        if w < 0.04 {
            continue;
        }
        let a = h * std::f32::consts::TAU;
        vec.0 += w * a.cos();
        vec.1 += w * a.sin();
        let i = ((h * BINS as f32) as usize).min(BINS - 1);
        bins[i].0 += w;
        bins[i].1 += w * s;
        sincos[i].0 += w * a.cos();
        sincos[i].1 += w * a.sin();
    }
    let total: f32 = bins.iter().map(|b| b.0).sum();
    let coloured = total / px.len().max(1) as f32;
    let tint_hue = vec.1.atan2(vec.0).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    // The vivid colour: the heaviest run of three bins, its hue and its
    // saturation the run's own (a run's middle bin may be empty).
    let run = |i: usize| [(i + BINS - 1) % BINS, i, (i + 1) % BINS];
    let accent = (0..BINS)
        .map(|i| (i, run(i).iter().map(|j| bins[*j].0).sum::<f32>()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, w)| coloured > 0.02 && *w > 0.0)
        .map(|(i, w)| {
            let (x, y) = run(i).iter().fold((0.0, 0.0), |(x, y), j| (x + sincos[*j].0, y + sincos[*j].1));
            let ws: f32 = run(i).iter().map(|j| bins[*j].1).sum();
            (y.atan2(x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU, ws / w)
        });
    // How much of the picture's hue the greys and the hues carry: a grey
    // picture gives plain greys.
    let tint = (coloured * 3.0).clamp(0.0, 1.0);
    // How vivid the picture is, for the hues: a muted one gives muted hues.
    let vivid = if total > 0.0 { bins.iter().map(|b| b.1).sum::<f32>() / total } else { 0.0 };
    let hue_s = (0.35 + vivid * 0.45).clamp(0.40, 0.70);
    let grey = |l: f32, s: f32| rgb(tint_hue, s * tint, l);
    let dark_greys = [grey(0.085, 0.28), grey(0.115, 0.26), grey(0.165, 0.22), grey(0.27, 0.18), grey(0.58, 0.12), grey(0.88, 0.10), grey(0.93, 0.08), grey(0.97, 0.06)];
    let light_greys = [grey(0.93, 0.30), grey(0.885, 0.26), grey(0.83, 0.22), grey(0.72, 0.18), grey(0.42, 0.12), grey(0.15, 0.14), grey(0.11, 0.12), grey(0.07, 0.10)];
    // Each hue leans toward the picture's by up to 18°, never past a slot.
    let lean = |h: f32| {
        let d = ((tint_hue - h + 0.5).rem_euclid(1.0)) - 0.5;
        h + d.clamp(-18.0 / 360.0, 18.0 / 360.0) * tint
    };
    let hue_set = |s: f32, l: f32, brown_l: f32| {
        let mut out = [[0u8; 3]; 8];
        for (i, h) in HUES.iter().enumerate() {
            out[i] = if i == 7 { rgb(lean(*h), s * 0.55, brown_l) } else { rgb(lean(*h), s, l) };
        }
        out
    };
    let mut dark_hues = hue_set(hue_s, 0.70, 0.62);
    let mut light_hues = hue_set(hue_s, 0.40, 0.36);
    // The accent: in the slot nearest its hue (not brown).
    let mut accent_slot = 0x0D;
    if let Some((h, s)) = accent {
        let slot = (0..7)
            .min_by(|a, b| {
                let d = |i: usize| (((HUES[i] - h + 0.5).rem_euclid(1.0)) - 0.5).abs();
                d(*a).total_cmp(&d(*b))
            })
            .unwrap_or(5);
        let s = s.clamp(0.45, 0.9);
        dark_hues[slot] = rgb(h, s, 0.70);
        light_hues[slot] = rgb(h, s, 0.40);
        accent_slot = 8 + slot;
    }
    // Every hue reads on the look's surface (3:1, what a sign needs): a
    // light green or yellow is walked darker in the light look.
    let fit = |hues: &mut [Rgb; 8], surface: Rgb, dark: bool| {
        for c in hues.iter_mut() {
            let (h, s, mut l) = hsl(*c);
            while contrast(rel_lum(*c), rel_lum(surface)) < 3.0 && (0.05..0.95).contains(&l) {
                l += if dark { 0.02 } else { -0.02 };
                *c = rgb(h, s, l);
            }
        }
    };
    fit(&mut dark_hues, dark_greys[1], true);
    fit(&mut light_hues, light_greys[1], false);
    let join = |g: [Rgb; 8], h: [Rgb; 8]| {
        let mut c = [[0u8; 3]; 16];
        c[..8].copy_from_slice(&g);
        c[8..].copy_from_slice(&h);
        c
    };
    let dark = join(dark_greys, dark_hues);
    Palette {
        id: DYNAMIC_ID.to_owned(),
        name: "Dynamic".to_owned(),
        soul: vec![dark[accent_slot], dark[7], dark[5], dark[3], dark[2], dark[1]],
        dark,
        light: join(light_greys, light_hues),
        accent: accent_slot,
        accent_light: accent_slot,
        family: "dynamic".to_owned(),
    }
}

// ------------------------------------------------------------ the tint

/// THE TINT (Max, 2026-10-16): how much of the palette a slider puts on the
/// grounds — base00..04 keep their lightness and go from no colour (1) through
/// the palette as made to its colour at full strength, the last tenth the
/// colour itself, saturated. The slider walks this path in EVEN VISIBLE steps
/// (the OKLab distance the grounds travel), so no stretch is dead and none
/// jumps. Grey palettes stay grey. The same maths as the approved mockup.
pub(crate) mod tint {
    use super::Rgb;

    fn lin(v: f32) -> f32 {
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }
    fn gam(v: f32) -> f32 {
        if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    }
    pub(crate) fn lab(c: Rgb) -> [f32; 3] {
        let [r, g, b] = c.map(|v| lin(f32::from(v) / 255.0));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }
    fn raw(l_: f32, a: f32, b: f32) -> [f32; 3] {
        let l = (l_ + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m = (l_ - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s = (l_ - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }
    pub(crate) fn from_lab(l: f32, a: f32, b: f32) -> Rgb {
        hex_of(raw(l, a, b))
    }
    fn hex_of(c: [f32; 3]) -> Rgb {
        c.map(|v| (gam(v.clamp(0.0, 1.0)) * 255.0).round() as u8)
    }
    pub(crate) fn lch(c: Rgb) -> (f32, f32, f32) {
        let [l, a, b] = lab(c);
        (l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
    }
    /// A hue of the wheel (`turn` 0..1), bright and clear: Dynamic's swatch.
    pub(crate) fn hue(turn: f32) -> Rgb {
        from_lch(0.74, 0.14, turn * 360.0)
    }

    /// OKLCH → sRGB, chroma reduced until it fits.
    pub(crate) fn from_lch(l: f32, mut c: f32, h: f32) -> Rgb {
        let (sin, cos) = h.to_radians().sin_cos();
        for _ in 0..40 {
            let rgb = raw(l, c * cos, c * sin);
            if rgb.iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v)) {
                return hex_of(rgb);
            }
            c *= 0.92;
        }
        hex_of(raw(l, c * cos, c * sin))
    }

    const MAXC: [f32; 5] = [0.100, 0.125, 0.135, 0.120, 0.085];

    /// The grounds at `u` on the path (0 no colour, 50 as made, 100 full).
    pub(crate) fn at(c: &[Rgb; 16], own: Rgb, u: f32, dark: bool) -> [Rgb; 16] {
        if (u - 50.0).abs() < 1e-3 {
            return *c;
        }
        let (_, ac, ah) = lch(own);
        let mut out = *c;
        for k in 0..5 {
            let (l, c0, h0) = lch(c[k]);
            let (mut cc, mut h, mut l2) = (c0, h0, l);
            if u < 50.0 {
                cc = c0 * u / 50.0;
            } else if ac > 0.03 {
                let f = (u - 50.0) / 50.0;
                cc = c0 + (MAXC[k] - c0) * f;
                h = h0 + (((ah - h0 + 540.0) % 360.0) - 180.0) * (f * 1.5).min(1.0);
                let w = if k < 3 { 1.0 } else { 0.5 };
                l2 = if dark { l + 0.075 * f * w } else { l - 0.06 * f * w };
            }
            if u > 90.0 && ac > 0.03 {
                let g = (u - 90.0) / 10.0;
                // (DARK IS DARKER — Max, 2026-10-18: full colour in the dark is a deep
                // tint, not a lit field: the grounds end near L* 12/16, not 27/31.)
                let lr = if dark { [0.21, 0.25, 0.31, 0.42, 0.82][k] } else { [0.86, 0.80, 0.74, 0.64, 0.36][k] };
                l2 += (lr - l2) * g;
                cc += (0.32 - cc) * g;
                h = ah;
            }
            out[k] = from_lch(l2, cc.max(0.0), h.rem_euclid(360.0));
        }
        out
    }

    /// The path's cumulative visible distance (101 steps) and where "as made" falls on the slider.
    fn path(c: &[Rgb; 16], own: Rgb, dark: bool) -> ([f32; 101], f32) {
        let mut cum = [0.0f32; 101];
        let mut prev: Option<[[f32; 3]; 4]> = None;
        for u in 0..=100 {
            let x = at(c, own, u as f32, dark);
            let labs = [lab(x[0]), lab(x[1]), lab(x[2]), lab(x[3])];
            if let Some(p) = prev {
                let d: f32 = (0..4).map(|k| ((labs[k][0] - p[k][0]).powi(2) + (labs[k][1] - p[k][1]).powi(2) + (labs[k][2] - p[k][2]).powi(2)).sqrt()).sum();
                cum[u] = cum[u - 1] + d / 4.0;
            }
            prev = Some(labs);
        }
        let total = cum[100].max(1e-6);
        (cum, 1.0 + cum[50] / total * 99.0)
    }

    /// Where "as made" falls on the slider (1..100) for this palette and look.
    #[cfg(test)]
    pub(crate) fn made_at(c: &[Rgb; 16], own: Rgb, dark: bool) -> f32 {
        path(c, own, dark).1
    }

    /// THE LIGHTING now (0..100, 50 as designed): set from the choice before a
    /// theme is made; every walk ends lit by it.
    static LIGHTING: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(50);
    pub(crate) fn set_lighting(v: u8) {
        LIGHTING.store(v.min(100), std::sync::atomic::Ordering::Relaxed);
    }
    #[allow(dead_code)]
    pub(crate) fn lighting() -> u8 {
        LIGHTING.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The dark look's grounds go no lighter than these, the light look's no
    /// darker than those (base00..03).
    const DARK_TOP: [f32; 4] = [0.30, 0.34, 0.39, 0.47];
    const LIGHT_FLOOR: [f32; 4] = [0.72, 0.68, 0.62, 0.55];

    /// The grounds (base00..03) moved by the Lighting `v`, within the look's
    /// band — the dark from black up to a dim grey, the light from soft paper
    /// up to white — hue and colour kept; the inks left (they read on every
    /// ground of the band). 50 changes nothing.
    pub(crate) fn lit(c: &[Rgb; 16], dark: bool, v: u8) -> [Rgb; 16] {
        if v == 50 {
            return *c;
        }
        let f = (f32::from(v.min(100)) - 50.0) / 50.0;
        let mut out = *c;
        for k in 0..4 {
            let (l, cc, h) = lch(c[k]);
            let l2 = if dark {
                (l + 0.14 * f).clamp(0.0, DARK_TOP[k])
            } else {
                // (Wide enough to be SEEN — Max: "it have to be noticeable, like a light
                // dimming"; ±0.06 moved a light box by 9/255 over a whole fade.)
                (l + 0.12 * f).clamp(LIGHT_FLOOR[k], 1.0)
            };
            out[k] = from_lch(l2, cc, h);
        }
        inks_read(c, out)
    }

    /// The inks of `c` walked to read on the new grounds of `out`.
    fn inks_read(c: &[Rgb; 16], mut out: [Rgb; 16]) -> [Rgb; 16] {
        // The inks keep what they read. The grounds moved, so every other
        // colour walks in lightness (hue and chroma kept), away from them,
        // until it reads on base00..02 as well as it did unlit (up to 7:1):
        // the Lighting never makes anything less readable than the palette
        // made it (2026-10-20: at 0 and 100 the dim ink fell under 3:1 on
        // buttons in every palette, links and the accent toward 2:1).
        use super::{contrast, rel_lum};
        let worst = |x: Rgb, g: &[Rgb; 16]| (0..3).map(|i| contrast(rel_lum(x), rel_lum(g[i]))).fold(f32::MAX, f32::min);
        for k in 4..16 {
            let want = worst(c[k], c).min(7.0);
            let up = rel_lum(c[k]) > rel_lum(out[1]);
            let (mut l, cc, h) = lch(c[k]);
            let mut x = c[k];
            let mut n = 0;
            while worst(x, &out) < want && n < 100 {
                l = (l + if up { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
                x = from_lch(l, cc, h);
                n += 1;
            }
            out[k] = x;
        }
        out
    }

    /// SATURATION: every colour's chroma scaled — 0 grey, 50 as made, 100
    /// twice as vivid (kept in gamut by `from_lch`). Lightness is kept, so
    /// what read on what still reads; the hues' floors are the caller's
    /// (`readable`, `as_text`).
    pub(crate) fn saturate(c: &[Rgb; 16], sat: u8) -> [Rgb; 16] {
        if sat == 50 {
            return *c;
        }
        let k = f32::from(sat.min(100)) / 50.0;
        let mut out = *c;
        for (i, x) in c.iter().enumerate() {
            let (l, ch, h) = lch(*x);
            out[i] = from_lch(l, ch * k, h);
        }
        out
    }

    /// The palette at a slider value: `None` as made; 0 (off) is the caller's.
    /// Lit by the surface's `light` and saturated by its `sat`.
    pub(crate) fn walk(c: &[Rgb; 16], own: Rgb, dark: bool, level: Option<u8>, light: u8, sat: u8) -> [Rgb; 16] {
        saturate(&lit(&walk_unlit(c, own, dark, level), dark, light), sat)
    }
    pub(crate) fn walk_unlit(c: &[Rgb; 16], own: Rgb, dark: bool, level: Option<u8>) -> [Rgb; 16] {
        let Some(s) = level.filter(|s| *s > 0) else { return *c };
        let (cum, made) = path(c, own, dark);
        if (f32::from(s) - made).abs() <= 1.5 {
            return *c;
        }
        let want = (f32::from(s) - 1.0) / 99.0 * cum[100];
        let mut u = 100.0;
        for i in 1..=100 {
            if cum[i] >= want {
                let (a, b) = (cum[i - 1], cum[i]);
                u = (i - 1) as f32 + if b > a { (want - a) / (b - a) } else { 0.0 };
                break;
            }
        }
        at(c, own, u, dark)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::palette::{contrast, rel_lum};

        #[test]
        fn the_path_goes_grey_made_full_and_stays_readable() {
            let p = crate::palette::all().into_iter().find(|p| p.id == "jade").expect("jade is in the box");
            let own = p.dark[p.accent];
            let grey = walk(&p.dark, own, true, Some(1), 50, 50);
            assert!(lch(grey[1]).1 < 0.004, "1 is neutral grey: {:?}", grey[1]);
            assert_eq!(walk(&p.dark, own, true, None, 50, 50), p.dark, "None is the palette as made");
            let made = made_at(&p.dark, own, true).round() as u8;
            assert_eq!(walk(&p.dark, own, true, Some(made), 50, 50), p.dark, "the as-made point gives the palette as made");
            let full = walk(&p.dark, own, true, Some(100), 50, 50);
            // (Dark is darker since 2026-10-18: the colour at depth — deep, but clearly it.)
            assert!(lch(full[1]).1 > 0.04 && lch(full[1]).0 < 0.3, "100 is the colour itself, deep: {:?}", full[1]);
            for v in [1u8, 25, 50, 75, 90, 100] {
                for (c, dark) in [(&p.dark, true), (&p.light, false)] {
                    let x = walk(c, own, dark, Some(v), 50, 50);
                    assert!(contrast(rel_lum(x[5]), rel_lum(x[1])) >= 4.5, "text reads at {v} (dark={dark})");
                }
            }
        }

        #[test]
        fn the_lighting_keeps_every_palette_readable_inside_its_band() {
            for p in crate::palette::all() {
                for v in [0u8, 25, 75, 100] {
                    for level in [None, Some(100)] {
                        let d = lit(&walk_unlit(&p.dark, p.dark[p.accent], true, level), true, v);
                        let l = lit(&walk_unlit(&p.light, p.light[p.accent_light], false, level), false, v);
                        assert!(contrast(rel_lum(d[5]), rel_lum(d[1])) >= 4.5, "{} dark at lighting {v}", p.id);
                        assert!(contrast(rel_lum(l[5]), rel_lum(l[1])) >= 4.5, "{} light at lighting {v}", p.id);
                        assert!(lch(d[0]).0 <= 0.306 && lch(l[0]).0 >= 0.714, "{} stays in its band at {v}", p.id);
                    }
                }
            }
            let p = crate::palette::all().into_iter().find(|p| p.id == "jade").unwrap();
            assert_eq!(lit(&p.dark, true, 50), p.dark, "50 is as designed");
            assert!(lch(lit(&p.dark, true, 0)[0]).0 < lch(p.dark[0]).0 + 1e-4, "less light: never lighter");
        }

        #[test]
        fn a_grey_palette_stays_grey() {
            let p = crate::palette::all().into_iter().find(|p| p.id == "charcoal").expect("charcoal is in the box");
            let x = walk(&p.dark, p.dark[p.accent], true, Some(100), 50, 50);
            assert!(lch(x[1]).1 < 0.01, "{:?}", x[1]);
        }
    }
}

// --------------------------------------------------------------- in use

static CURRENT: RwLock<Option<Arc<Theme>>> = RwLock::new(None);
static DYNAMIC: AtomicBool = AtomicBool::new(true);

/// The theme in use (Golem's dark before the first [`set`]).
pub(crate) fn current() -> Arc<Theme> {
    if let Some(t) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return t.clone();
    }
    Arc::new(resolve(&ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }, true))
}

/// Whether the shell colours itself from the screen (the default).
pub(crate) fn dynamic() -> bool {
    DYNAMIC.load(Ordering::Relaxed)
}

/// Put a theme in use. True when it is a change.
pub(crate) fn set(theme: Theme) -> bool {
    DYNAMIC.store(theme.dynamic, Ordering::Relaxed);
    let mut cur = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    if cur.as_deref() == Some(&theme) {
        return false;
    }
    *cur = Some(Arc::new(theme));
    true
}

/// A colour of the palette as plain sRGB fractions, opaque — the space the
/// shell's fixed signal colours were written in (the bell's chip, the reds).
pub(crate) fn srgb(c: Rgb) -> [f32; 4] {
    [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, 1.0]
}

/// The accent, the palette's red and its yellow as [`srgb`]: what the shell's
/// chips, warnings and "this cannot be undone" wear (Max, 2026-10-15: the
/// palette "as deep as possible"). Golem's own palette gives back exactly
/// the colours these were before (#ffbe98, #e05252).
/// (As words — the shell writes in them — text-safe: `as_text` on the
/// palette's grounds, then on the shell's own surface as lit and as the
/// night light dims it (`shell_words`); a colour that reads stays as it is.)
pub(crate) fn accent_srgb() -> [f32; 4] {
    srgb(shell_words(signals().accent_text()))
}
pub(crate) fn danger_srgb() -> [f32; 4] {
    let t = signals();
    srgb(shell_words(t.as_text(t.c[0x08])))
}

/// HOW DARK A WINDOW OUT OF FOCUS GOES (Hyprland's `dim_strength`; Max,
/// 2026-10-23, after the legibility audit showed 0.3 taking almost every
/// pair under 4.5:1): 15% while the apps are Light — black over a light
/// window cuts its contrast the most — 25% while they are Dark.
pub(crate) fn dim_inactive_for(apps_dark: bool) -> f32 {
    if apps_dark { 0.25 } else { 0.15 }
}

/// `x` readable on the shell's surface as it is drawn: lit by the Shell's
/// Lighting, and dimmed by the night light (`options::NIGHT_DIM`) — the
/// accent on a box at Lighting 100 read 4.4:1 (the legibility audit).
pub(crate) fn shell_words(x: Rgb) -> Rgb {
    let Some((surface, _)) = shell() else { return x };
    let to = |c: [f32; 4]| -> Rgb { [0, 1, 2].map(|i| (crate::options::linear_to_srgb(c[i]).clamp(0.0, 1.0) * 255.0).round() as u8) };
    let dim = 1.0 - crate::options::NIGHT_DIM;
    readable_on(x, &[to(surface), to([surface[0] * dim, surface[1] * dim, surface[2] * dim, 1.0])])
}

/// `x` walked in lightness, hue and chroma kept, until it reads 4.5:1 on
/// each of `grounds` (away from the first).
pub(crate) fn readable_on(x: Rgb, grounds: &[Rgb]) -> Rgb {
    let r = |a: Rgb, b: Rgb| contrast(rel_lum(a), rel_lum(b));
    let Some(first) = grounds.first() else { return x };
    let up = rel_lum(x) > rel_lum(*first);
    let (mut l, ch, h) = tint::lch(x);
    let mut y = x;
    let mut n = 0;
    while grounds.iter().any(|g| r(y, *g) < 4.5) && n < 100 {
        l = (l + if up { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
        y = tint::from_lch(l, ch, h);
        n += 1;
    }
    y
}
pub(crate) fn warn_srgb() -> [f32; 4] {
    srgb(signals().c[0x0A])
}

/// Whose colours the shell's signals wear: the palette's while the Shell
/// slider takes colour, GOLEM'S OWN at Off (Max, 2026-10-17: "i have the
/// shell off on the slider, and it is still red" — a Dynamic palette from a
/// desert wallpaper had made the bell, the chips and the card red).
fn signals() -> Arc<Theme> {
    if SHELL_COLOURED.load(Ordering::Relaxed) {
        return current();
    }
    static OWN: [std::sync::OnceLock<Arc<Theme>>; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
    // (Drawn every frame: the look is the flag `set_shell` keeps, never a
    // read of the settings file here.)
    let dark = SHELL_IS_DARK.load(Ordering::Relaxed);
    OWN[usize::from(dark)]
        .get_or_init(|| Arc::new(resolve(&ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }, dark)))
        .clone()
}

/// The Shell slider is above Off.
static SHELL_COLOURED: AtomicBool = AtomicBool::new(true);
/// The shell's own look, as `set_shell` last resolved it.
static SHELL_IS_DARK: AtomicBool = AtomicBool::new(true);

/// A colour of the palette as the renderer wants it: linear, opaque.
pub(crate) fn linear(c: Rgb) -> [f32; 4] {
    let f = |v: u8| crate::options::srgb_to_linear(f32::from(v) / 255.0);
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

pub(crate) fn rel_lum(c: Rgb) -> f32 {
    let l = linear(c);
    0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
}

pub(crate) fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// ------------------------------------------------------------ the apps

/// Write the theme out for everything outside the daemon and tell the
/// running apps — on a thread of its own: it runs programs.
pub(crate) fn publish(theme: Arc<Theme>, look_changed: bool) {
    // (Counted HERE, before the writer's lock: counted inside, a newer change
    // could not count until the older had finished, and a drag of the slider
    // rewrote the apps' icons once a step.)
    let mine = ICONS_ASKED.fetch_add(1, Ordering::Relaxed) + 1;
    let spawned = std::thread::Builder::new().name("palette".into()).spawn(move || {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        // A newer change is waiting its turn: this one's files would be
        // written only to be written over (a drag of a slider is many
        // changes; each rewrote everything and reloaded every GTK app).
        if ICONS_ASKED.load(Ordering::Relaxed) != mine {
            return;
        }
        // What the programs wear may be what they wear already (a change of
        // the web switch only): then only the shared file is written, and no
        // app is told to reload (GTK's theme flipped name on every switch).
        static LAST: std::sync::Mutex<Option<Theme>> = std::sync::Mutex::new(None);
        let apps = for_apps(&theme);
        let same = {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            let same = last.as_ref().is_some_and(|l| same_for_apps(l, &apps));
            *last = Some(apps.clone());
            same
        };
        if same {
            write_shared(&theme_dir(), &theme);
        } else {
            write_all(&theme_dir(), &theme);
            poke(&apps, look_changed);
            // (After the poke: the GTK theme in use has just turned name, and
            // Inkscape's fixed theme points at the new one.)
            crate::app_themes::write(&apps);
        }
        // (The apps' icons once the slider rests: a drag is many changes.)
        std::thread::sleep(std::time::Duration::from_millis(400));
        if ICONS_ASKED.load(Ordering::Relaxed) == mine {
            publish_icons(mine);
        }
        info!("palette: {} ({}), accent {}", theme.name, if theme.dark { "dark" } else { "light" }, slot_name(theme.accent_slot));
    });
    if let Err(e) = spawned {
        warn!("palette: no thread to write the theme ({e})");
    }
}

// ------------------------------------------------------- the apps' icons

/// THE ICONS IN EVERY APP (Max, 2026-10-17: "the icons on thunar are not
/// tinted… we cant have this inconsistency in any app"). The dock tints its
/// icons in its shader; the apps read the icon theme from disk. So while the
/// Icons slider is above Off (or its Dark is on), Golem's theme is written
/// again through the SAME filter (`filter_rgb`, the shader's sums on the hex
/// colours of each drawing) as `Golem-Tinted-a|b` in the data home, and
/// that is made the system's icon theme. The name turns a↔b at each change:
/// an app reloads its icons only when the theme's NAME changes. At Off the
/// plain theme is put back and the copies go. (The dock itself keeps the
/// plain theme and its shader: nothing is tinted twice.)
static ICONS_ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn publish_icons(mine: u64) {
    let f = icon_filter();
    let (k, dim) = (f[0][0], f[0][1] > 0.5);
    // (The icons' tone is the Icons row's look: graphite in the dark, silver in the light.)
    // (The look in force: Auto's and Ambient's included, not only the row's own.)
    let base = if dim || current().icons_dark { "Golem-Dark" } else { "Golem" };
    let home = std::env::var("HOME").unwrap_or_default();
    let data = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let icons = std::path::Path::new(&data).join("icons");
    let names = ["Golem-Tinted-a", "Golem-Tinted-b"];
    let dconf = |name: &str| {
        // (A nested test dock names no icon theme to the owner's session.)
        if rig() {
            return;
        }
        run("dconf", &["write", "/org/gnome/desktop/interface/icon-theme", &format!("'{name}'")]);
        // Qt apps are told the same name at the same moment (their settings
        // file, where it is ours): a name told earlier could be one already
        // gone.
        for app in ["qt5ct", "qt6ct"] {
            let conf = config_home().join(app).join(format!("{app}.conf"));
            let Ok(text) = std::fs::read_to_string(&conf) else { continue };
            if std::fs::symlink_metadata(&conf).is_ok_and(|m| m.file_type().is_symlink()) || !text.contains("golem/theme/qt") {
                continue;
            }
            let text: String = text
                .lines()
                .map(|l| if l.starts_with("icon_theme=") { format!("icon_theme={name}\n") } else { format!("{l}\n") })
                .collect();
            crate::persist::write_text("theme", &conf, &text);
        }
    };
    if !crate::look::installed(base) {
        return;
    }
    if k <= 0.0 && !dim {
        dconf(base);
        for n in names {
            let _ = std::fs::remove_dir_all(icons.join(n));
        }
        return;
    }
    let Some(src) = icon_theme_dir(base) else { return };
    let small = if base == "Golem-Dark" { "Papirus-Dark" } else { "Papirus" };
    // (Written already for these very colours — a dock start: kept. The
    // drawings are named by their packages — where each theme's index really
    // lives — not by the profile around them, which every update renews: on
    // the Acer's disk that rewrite read cold for 80 s after each update.)
    let package = |dir: Option<std::path::PathBuf>| {
        dir.and_then(|d| std::fs::canonicalize(d.join("index.theme")).ok()).map(|p| p.display().to_string()).unwrap_or_default()
    };
    let stamp = format!("{base} {} {} {f:?} v4", package(Some(src.clone())), package(icon_theme_dir(small)));
    let now_dir = names.iter().map(|n| icons.join(n)).find(|d| d.is_dir());
    if let Some(d) = &now_dir {
        if std::fs::read_to_string(d.join(".golem-stamp")).ok().as_deref() == Some(stamp.as_str()) {
            dconf(&d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            return;
        }
    }
    // The one not in use now gets the new drawings (in use = the one dconf
    // names; failing that, the one with a stamp).
    let in_use = out("dconf", &["read", "/org/gnome/desktop/interface/icon-theme"]).map(|o| o.trim().trim_matches('\'').to_owned());
    let now = names
        .iter()
        .find(|n| in_use.as_deref() == Some(**n))
        .or_else(|| names.iter().find(|n| icons.join(n).join(".golem-stamp").is_file()))
        .copied();
    let (next, old) = if now == Some(names[0]) { (names[1], names[0]) } else { (names[0], names[1]) };
    let dest = icons.join(next);
    let _ = std::fs::remove_dir_all(&dest);
    let started = std::time::Instant::now();
    let mut seen = std::collections::HashSet::new();
    // A newer change gives up this one where it is: the writer's lock is
    // held meanwhile, and on a cold disk this is a minute (the Acer: 78 s)
    // the apps' colours would wait behind.
    let newer = || ICONS_ASKED.load(Ordering::Relaxed) != mine;
    if let Err(e) = tint_tree(&src, &dest, &f, next, 0, &mut seen, &newer).and_then(|()| add_small_papirus(&dest, small, &f, &newer)) {
        if e.kind() == std::io::ErrorKind::Interrupted {
            info!("palette: the apps' icons left for a newer change");
        } else {
            warn!("palette: the apps' icons were not written ({e})");
        }
        let _ = std::fs::remove_dir_all(&dest);
        return;
    }
    let _ = std::fs::write(dest.join(".golem-stamp"), &stamp);
    info!("palette: the apps' icons written in {} ms", started.elapsed().as_millis());
    dconf(next);
    let _ = std::fs::remove_dir_all(icons.join(old));
}

/// What the apps show SMALL that Golem does not draw — the menus' and the
/// side panels' icons (Max, 2026-10-17: "a few on the menus of thunar"):
/// Papirus's places, devices, actions and apps at 16 and 24 px, tinted the
/// same way, as directories of the tinted theme. A name Golem draws is left
/// to Golem's; a symbolic icon to GTK (it colours those with the text). Each
/// drawing is tinted once; its other names are hard links to it.
fn add_small_papirus(dest: &std::path::Path, variant: &str, f: &[[f32; 4]; 7], newer: &dyn Fn() -> bool) -> std::io::Result<()> {
    let Some(src) = icon_theme_dir(variant) else { return Ok(()) };
    // The names Golem's own theme has.
    let mut golem = std::collections::HashSet::new();
    let mut stack = vec![dest.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                golem.insert(n.to_owned());
            }
        }
    }
    let mut done: std::collections::HashMap<std::path::PathBuf, std::path::PathBuf> = std::collections::HashMap::new();
    let mut sections = String::new();
    let mut dirs = Vec::new();
    for size in [16, 24] {
        for (ctx, context) in [("places", "Places"), ("devices", "Devices"), ("actions", "Actions"), ("apps", "Applications")] {
            let from = src.join(format!("{size}x{size}")).join(ctx);
            let Ok(list) = std::fs::read_dir(&from) else { continue };
            let rel = format!("small/{size}/{ctx}");
            let to = dest.join(&rel);
            std::fs::create_dir_all(&to)?;
            for e in list.flatten() {
                if newer() {
                    return Err(given_up());
                }
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".svg") || name.contains("-symbolic") || golem.contains(&name) {
                    continue;
                }
                let Ok(real) = std::fs::canonicalize(e.path()) else { continue };
                let out = to.join(&name);
                if let Some(first) = done.get(&real) {
                    if std::fs::hard_link(first, &out).is_ok() {
                        continue;
                    }
                }
                let Ok(text) = std::fs::read_to_string(&real) else { continue };
                std::fs::write(&out, tint_svg(&text, f))?;
                done.insert(real, out);
            }
            sections.push_str(&format!("\n[{rel}]\nContext={context}\nSize={size}\nType=Fixed\n"));
            dirs.push(rel);
        }
    }
    // The theme's index names the new directories.
    let index = dest.join("index.theme");
    let text = std::fs::read_to_string(&index)?;
    let text: String = text
        .lines()
        .map(|l| if l.starts_with("Directories=") { format!("{l},{}", dirs.join(",")) } else { l.to_owned() })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&index, text + "\n" + &sections)?;
    Ok(())
}

/// A tinting given up for a newer change.
fn given_up() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "a newer change")
}

/// Where an installed icon theme lives.
fn icon_theme_dir(name: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    std::iter::once(data_home)
        .chain(dirs.split(':').map(str::to_owned))
        .map(|d| std::path::Path::new(&d).join("icons").join(name))
        .find(|p| p.join("index.theme").is_file())
}

/// Copy a theme's tree, its drawings' colours through the filter.
fn tint_tree(
    src: &std::path::Path,
    dest: &std::path::Path,
    f: &[[f32; 4]; 7],
    name: &str,
    depth: u8,
    seen: &mut std::collections::HashSet<std::path::PathBuf>,
    newer: &dyn Fn() -> bool,
) -> std::io::Result<()> {
    // (An icon theme is a few folders deep: anything deeper is not one. A
    // folder is walked once, wherever a link leads back to it.)
    if depth > 6 || !seen.insert(std::fs::canonicalize(src)?) {
        return Ok(());
    }
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)? {
        if newer() {
            return Err(given_up());
        }
        let e = e?;
        let (from, to) = (e.path(), dest.join(e.file_name()));
        let kind = std::fs::symlink_metadata(&from)?.file_type();
        // A RELATIVE link to a file is kept a link (an alias of a drawing —
        // tinted once, where it points). Anything else a link leads to is
        // walked as what it is: the installed theme's own folders are links
        // into the store (home-manager's), and skipping them left the tinted
        // theme without Golem's icons (the second review, 2026-10-17).
        let meta = std::fs::metadata(&from)?;
        if kind.is_symlink() && !meta.is_dir() {
            if let Ok(target) = std::fs::read_link(&from) {
                if target.is_relative() && !target.to_string_lossy().contains("..") {
                    let _ = std::os::unix::fs::symlink(&target, &to);
                    continue;
                }
            }
        }
        if meta.is_dir() {
            tint_tree(&from, &to, f, name, depth + 1, seen, newer)?;
        } else if e.file_name() == "icon-theme.cache" {
            // (It describes the source's tree, not this one.)
        } else if from.extension().is_some_and(|x| x == "svg") {
            match std::fs::read_to_string(&from) {
                Ok(text) => std::fs::write(&to, tint_svg(&text, f))?,
                // (Not text as it should be: as it is, untinted.)
                Err(_) => {
                    std::fs::copy(&from, &to)?;
                }
            }
        } else if e.file_name() == "index.theme" {
            let text = std::fs::read_to_string(&from)?;
            let text: Vec<String> = text
                .lines()
                .map(|l| if l.starts_with("Name=") { format!("Name={name}") } else { l.to_owned() })
                .collect();
            std::fs::write(&to, text.join("\n") + "\n")?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Every `#rrggbb` (and `#rgb`) of a drawing through the filter.
fn tint_svg(svg: &str, f: &[[f32; 4]; 7]) -> String {
    let b = svg.as_bytes();
    let mut out = String::with_capacity(svg.len());
    let mut i = 0;
    let hexd = |c: u8| (c as char).to_digit(16);
    while i < b.len() {
        // (Not a colour: an entity `&#160;`, a link `url(#a1b2c3)` or
        // `href="#abc"`.)
        let link = i > 0 && matches!(b[i - 1], b'&' | b'(' | b'"' | b'\'') && {
            let before = &svg[..i];
            before.ends_with('&') || before.ends_with("url(") || before.ends_with("href=\"") || before.ends_with("href='")
        };
        if b[i] == b'#' && !link {
            let six = i + 7 <= b.len() && b[i + 1..i + 7].iter().all(|c| hexd(*c).is_some())
                && !(i + 7 < b.len() && hexd(b[i + 7]).is_some());
            let three = !six && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| hexd(*c).is_some())
                && !(i + 4 < b.len() && hexd(b[i + 4]).is_some());
            if six || three {
                let n = if six { 6 } else { 3 };
                let d: Vec<u8> = b[i + 1..i + 1 + n].iter().map(|c| hexd(*c).unwrap_or(0) as u8).collect();
                let rgb = if six { [d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5]] } else { [d[0] * 17, d[1] * 17, d[2] * 17] };
                let t = filter_rgb(rgb, f);
                out.push_str(&format!("#{:02x}{:02x}{:02x}", t[0], t[1], t[2]));
                i += 1 + n;
                continue;
            }
        }
        // (Copied a character at a time: a drawing may hold any UTF-8.)
        let ch = svg[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The icon shader's filter (`icon.wgsl`, `icon_filter`) on one colour.
pub(crate) fn filter_rgb(rgb: Rgb, f: &[[f32; 4]; 7]) -> Rgb {
    let (k, dim, n) = (f[0][0], f[0][1] > 0.5, f[0][2] as usize);
    let mut out = rgb;
    if n > 0 && k > 0.0 {
        let (l, c, h) = tint::lch(rgb);
        let targets = &f[1..=n.min(6)];
        let painted = if c < 0.03 {
            let best = targets.iter().fold(targets[0], |a, t| if t[1] > a[1] { *t } else { a });
            let mid = (4.0 * l * (1.0 - l)).clamp(0.0, 1.0);
            tint::from_lch(l, (best[1] * 0.6 * mid).max(0.035), best[2].to_degrees())
        } else {
            let hr = h.to_radians();
            let best = targets
                .iter()
                .min_by(|a, b| {
                    let d = |t: &[f32; 4]| (t[2] - hr).sin().atan2((t[2] - hr).cos()).abs();
                    d(a).total_cmp(&d(b))
                })
                .copied()
                .unwrap_or(targets[0]);
            tint::from_lch(l * 0.5 + best[0] * 0.5, best[1], best[2].to_degrees())
        };
        // Mixed in sRGB, as the shader does.
        out = std::array::from_fn(|i| (f32::from(rgb[i]) + (f32::from(painted[i]) - f32::from(rgb[i])) * k).round() as u8);
    }
    if dim {
        let (l, c, h) = tint::lch(out);
        let l2 = if l > 0.45 { 0.45 + (l - 0.45) * 0.55 } else { l * 0.96 };
        out = tint::from_lch(l2, c * 0.92, h);
    }
    out
}

/// Whether two themes dress the programs alike: the same colours, accent and
/// look (the web switch and the name aside).
fn same_for_apps(a: &Theme, b: &Theme) -> bool {
    a.id == b.id && a.c == b.c && a.dark == b.dark && a.accent_slot == b.accent_slot
}

/// Only the file every reader shares (`colors.json`: Seam's web switches).
fn write_shared(dir: &Path, shell: &Theme) {
    let t = &for_apps(shell);
    crate::persist::write_text("theme", &dir.join("colors.json"), &colors_json(t, shell, &choice(), &crate::webapps::app_hosts()));
}

/// What the programs wear: the palette walked to the Apps slider in the apps'
/// own look; at 0 (off) the makers' own colours, the palette's accent kept
/// (Max, 2026-10-16: off leaves the grounds, the accent still paints).
pub(crate) fn for_apps(t: &Theme) -> Theme {
    let pal = palette_of(&t.id).unwrap_or_else(golem);
    let own = pal.dark[pal.accent];
    let (dark_v, light_v) = match t.apps_tint {
        Some(0) => {
            let st = parse("stock", STOCK).expect("the Stock palette parses");
            let (mut d, mut l) = (st.dark, st.light);
            d[st.accent] = pal.dark[pal.accent];
            l[st.accent] = pal.light[pal.accent_light];
            readable(&mut d, true, st.accent);
            readable(&mut l, false, st.accent);
            // (the palette's accent goes where the makers keep theirs)
            let pair = match (t.apps_dark != t.dark, t.apps_dark) {
                (true, true) => (d, st.accent, d, st.accent),
                (true, false) => (l, st.accent, l, st.accent),
                _ => (d, st.accent, l, st.accent),
            };
            return Theme {
                c: if t.apps_dark { d } else { l },
                accent_slot: st.accent,
                dark: t.apps_dark,
                pair: Some(pair),
                looks: Some((d, st.accent, l, st.accent)),
                ..t.clone()
            };
        }
        level => (tint::walk(&pal.dark, own, true, level, t.apps_light, t.apps_sat), tint::walk(&pal.light, own, false, level, t.apps_light, t.apps_sat)),
    };
    let (mut dark_v, mut light_v) = (dark_v, light_v);
    readable(&mut dark_v, true, pal.accent);
    readable(&mut light_v, false, pal.accent_light);
    let looks = (dark_v, pal.accent, light_v, pal.accent_light);
    let mut pair = looks;
    if t.apps_dark != t.dark {
        // The apps keep a look of their own: both halves wear it, since
        // GTK 4 picks its half by the SYSTEM's look (prefers-color-scheme).
        pair = if t.apps_dark { (dark_v, pal.accent, dark_v, pal.accent) } else { (light_v, pal.accent_light, light_v, pal.accent_light) };
    }
    Theme {
        c: if t.apps_dark { dark_v } else { light_v },
        accent_slot: if t.apps_dark { pal.accent } else { pal.accent_light },
        dark: t.apps_dark,
        pair: Some(pair),
        looks: Some(looks),
        ..t.clone()
    }
}

/// The programs (the first word of their command) that keep their own
/// colours: an app the dock starts gets the makers' themes in its
/// environment ([`skip_vars`]).
static SKIPPED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The programs with a look of their own (the Apps page's Dark or Light —
/// the Colours model, 2026-10-20): program -> dark.
static LOOKED: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());

/// The two GTK themes a program pinned to a look starts with: the palette in
/// that look, always there under fixed names (`gtk3_live` keeps them with
/// the one in use).
const GTK3_PINNED: [&str; 2] = ["Golem-Dark", "Golem-Light"];

fn program_of(exec: &str) -> String {
    let first = exec.split_whitespace().find(|w| !w.contains('=') && *w != "env").unwrap_or("");
    first.rsplit('/').next().unwrap_or("").to_owned()
}

/// The apps' own colours and looks as programs, for the entries there are now.
pub(crate) fn exceptions_follow(entries: &[waverunner_core::index::AppEntry]) {
    let c = choice();
    set_skipped_programs(entries, c.skip_in_force());
    set_looked_programs(entries, c.looks_in_force());
}

/// An app's own look (`ColorChoice::app_look`, by desktop id) as programs.
pub(crate) fn set_looked_programs(entries: &[waverunner_core::index::AppEntry], looks: &std::collections::BTreeMap<String, String>) {
    let progs: Vec<(String, bool)> = entries
        .iter()
        .filter(|e| !e.id.starts_with("webapp-"))
        .filter_map(|e| looks.get(&e.id).map(|l| (program_of(&e.exec), l == "dark")))
        .filter(|(p, _)| !p.is_empty())
        .collect();
    if let Ok(mut g) = LOOKED.lock() {
        *g = progs;
    }
}

pub(crate) fn set_skipped_programs(entries: &[waverunner_core::index::AppEntry], skip: &[String]) {
    let progs: Vec<String> = entries
        .iter()
        .filter(|e| skip.contains(&e.id) && !e.id.starts_with("webapp-"))
        .map(|e| program_of(&e.exec))
        .filter(|p| !p.is_empty())
        .collect();
    if let Ok(mut g) = SKIPPED.lock() {
        *g = progs;
    }
}

/// What a program kept in its own colours is started with: GTK's own
/// Adwaita (in the apps' look) over our theme, Qt without qt5ct/qt6ct.
pub(crate) fn skip_vars(exec: &str) -> Vec<(&'static str, String)> {
    let p = program_of(exec);
    let own = SKIPPED.lock().map(|g| g.contains(&p)).unwrap_or(false);
    let looked = LOOKED.lock().ok().and_then(|g| g.iter().find(|(q, _)| *q == p).map(|(_, d)| *d));
    if !own && looked.is_none() {
        return Vec::new();
    }
    let dark = looked.unwrap_or_else(|| current().apps_dark);
    if own {
        return vec![("GTK_THEME", if dark { "Adwaita:dark" } else { "Adwaita" }.to_owned()), ("QT_QPA_PLATFORMTHEME", String::new())];
    }
    // The palette in a look of its own (GTK: the pinned theme of that look;
    // libadwaita takes the scheme from GTK_THEME's ":dark" too). Qt keeps the
    // Apps' look: its colours are one file for every program.
    let name = GTK3_PINNED[usize::from(!dark)];
    vec![("GTK_THEME", if dark { format!("{name}:dark") } else { name.to_owned() })]
}

/// THE ICON FILTER (Max, 2026-10-16: "more aggressive"), for the icon
/// shader: row 0 = (strength 0..1, dark icons 0/1, how many targets, _),
/// rows 1..=6 = the palette's strongest colours as OKLCH (L, C, hue in
/// radians). Each icon colour goes to the nearest of them by hue, at their
/// full chroma; a grey takes a whisper of the strongest.
static ICON_FILTER: std::sync::Mutex<[[f32; 4]; 7]> = std::sync::Mutex::new([[0.0; 4]; 7]);

pub(crate) fn icon_filter() -> [[f32; 4]; 7] {
    ICON_FILTER.lock().map(|f| *f).unwrap_or([[0.0; 4]; 7])
}

/// THE SHELL'S COLOURS (the dock and the boxes): its surface and its ink,
/// linear — the palette's variant in the shell's own look, walked to the
/// Shell slider. None: the shell reads the screen (a Dynamic palette with
/// the slider above Off); at Off, Golem's own neutral ground.
static SHELL: std::sync::Mutex<Option<([f32; 4], [f32; 4])>> = std::sync::Mutex::new(None);

/// The shell's own look: its Dark/Light (the sun's under Ambient), else the system's.
pub(crate) fn shell_dark() -> bool {
    choice().levels_now(Surface::Shell).dark
}

pub(crate) fn shell() -> Option<([f32; 4], [f32; 4])> {
    SHELL.lock().ok().and_then(|g| *g)
}

fn set_shell(picked: &ColorChoice, system_dark: bool) {
    let lv = picked.levels(Surface::Shell, crate::lighting::ambient_line(), system_dark);
    SHELL_COLOURED.store(lv.tint != Some(0), Ordering::Relaxed);
    SHELL_IS_DARK.store(lv.dark, Ordering::Relaxed);
    store_shell(shell_plan(picked, system_dark).pair(lv.light));
}

/// Put the shell's pair in use (None: it reads the screen).
pub(crate) fn store_shell(out: Option<([f32; 4], [f32; 4])>) {
    if let Ok(mut g) = SHELL.lock() {
        *g = out;
    }
}

/// How the shell's pair is made, split from the Lighting so a fade of the
/// Lighting (`lighting.rs`: 20 s, ~15 frames a second) can make the pair at
/// each value without finding the palette, walking its path or reading the
/// settings again: that is done once, here.
pub(crate) enum ShellPlan {
    /// The shell reads the screen (the Dynamic palette): no pair.
    Screen,
    /// A pair the Lighting does not move (Off: Golem's own neutral ground).
    Fixed(([f32; 4], [f32; 4])),
    /// The palette's variant walked to the Shell slider, not yet lit; its Saturation.
    Walked { unlit: [Rgb; 16], dark: bool, sat: u8 },
}

impl ShellPlan {
    /// The surface and the ink, linear, at the Lighting `v`.
    pub(crate) fn pair(&self, v: u8) -> Option<([f32; 4], [f32; 4])> {
        match self {
            Self::Screen => None,
            Self::Fixed(p) => Some(*p),
            // (`readable` moves only the hues, slots 8..16: neither the
            // surface nor the ink, so it is not run for every frame.)
            Self::Walked { unlit, dark, sat } => {
                let c = tint::saturate(&tint::lit(unlit, *dark, v), *sat);
                Some((linear(c[0x01]), linear(c[0x05])))
            }
        }
    }

    /// Whether the Lighting moves the shell's colours at all.
    pub(crate) fn follows_lighting(&self) -> bool {
        matches!(self, Self::Walked { .. })
    }
}

pub(crate) fn shell_plan(picked: &ColorChoice, system_dark: bool) -> ShellPlan {
    shell_plan_look(picked, picked.levels(Surface::Shell, crate::lighting::ambient_line(), system_dark).dark)
}

/// [`shell_plan`] in a look given: a fade across the looks keeps one of each.
pub(crate) fn shell_plan_look(picked: &ColorChoice, dark: bool) -> ShellPlan {
    let want = picked.palette_id();
    let lv = Levels { dark, ..picked.levels(Surface::Shell, None, dark) };
    if !lv.dark {
        // THE LIGHT SHELL IS THE DESIGNED GLASS (Max, 2026-10-21: "the shell
        // should not go white, we design the dark and light shell. and none
        // of them were white"): in the Light look the shell reads the screen,
        // as it was made to. A palette's light ground is the paper made for
        // windows — Chartreuse #e3e5da, Golem's own #dcd9d1 — and painted on
        // the shell it went white. (The palette still gives the shell its
        // signal colours; the Dark look still wears the palette's ground.)
        ShellPlan::Screen
    } else if lv.tint == Some(0) {
        // OFF is neutral (Max, 2026-10-17: the dock box and the Super+H
        // boxes "have a reddish ground" — they sampled a desert wallpaper):
        // Golem's own ground and ink, in the shell's Dark/Light.
        let t = resolve(&ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }, lv.dark);
        ShellPlan::Fixed((linear(t.surface()), linear(t.ink())))
    } else if want == DYNAMIC_ID {
        ShellPlan::Screen
    } else {
        let p = if want == DRAFT_ID { preview() } else { all().into_iter().find(|p| p.id == want) }.unwrap_or_else(golem);
        let base = if lv.dark { p.dark } else { p.light };
        ShellPlan::Walked { unlit: tint::walk_unlit(&base, p.dark[p.accent], lv.dark, lv.tint), dark: lv.dark, sat: lv.sat }
    }
}

fn set_icon_filter(t: &Theme) {
    // Dark icons dim at Off too when their Dark was clicked (Off = no
    // colour, not no dark).
    let c = choice();
    let dim = if t.icons_tint > 0 { t.icons_dark } else { t.icons_dark && (c.icons_dark.is_some() || c.ambient_of(Surface::Icons)) };
    let f = filter_for(&t.id, t.icons_tint, dim, t.icons_sat);
    if let Ok(mut g) = ICON_FILTER.lock() {
        *g = f;
    }
}

/// A filter (the icon shader's layout, see [`icon_filter`]) for the palette
/// `id` at `level` 0..100: the icons' and the wallpapers' alike. Level 0 and
/// no `dim` is the identity. `sat` (0 grey, 50 as made, 100 vivid) rides in
/// row 0's last slot as a chroma factor for the shaders.
pub(crate) fn filter_for(id: &str, level: u8, dim: bool, sat: u8) -> [[f32; 4]; 7] {
    let mut f = [[0.0f32; 4]; 7];
    if let Some(p) = palette_of(id) {
        let soul: Vec<Rgb> = if p.soul.is_empty() { p.dark[8..16].to_vec() } else { p.soul.clone() };
        let all: Vec<(f32, f32, f32)> = soul.iter().map(|&c| tint::lch(c)).collect();
        let cmax = all.iter().map(|t| t.1).fold(0.0f32, f32::max);
        let targets: Vec<_> = all.into_iter().filter(|t| t.1 > 0.03 && t.1 >= cmax * 0.55).take(6).collect();
        f[0] = [f32::from(level) / 100.0, if dim { 1.0 } else { 0.0 }, targets.len() as f32, 0.0];
        for (k, (l, c, h)) in targets.into_iter().enumerate() {
            f[k + 1] = [l, c, h.to_radians(), 0.0];
        }
    }
    // (Last: row 0 is written whole above, and the Saturation was lost with
    // it — the wallpapers' Saturation never reached them, 2026-10-21. Grey is
    // a hair above 0: an empty filter's 0 reads "as made", `wall_tint::sat_of`.)
    f[0][3] = (f32::from(sat.min(100)) / 50.0).max(0.001);
    f
}

/// THE WALLPAPERS' FILTER now (the palette in use at the Colours slider of
/// the Wallpaper page), built fresh from the saved choice: for a thread that
/// may wait. Level 0 is the identity.
pub(crate) fn wall_filter_now() -> [[f32; 4]; 7] {
    let c = choice();
    let lw = c.levels_now(Surface::Wall);
    let level = lw.tint.unwrap_or(0).min(100);
    if level == 0 && lw.sat == 50 {
        return [[0.0; 4]; 7];
    }
    filter_for(if previewing() { DRAFT_ID } else { c.palette_id() }, level, false, lw.sat)
}

/// The wallpapers' filter as `colors_apply` last set it — what the box's
/// thumbnails are drawn through (a cheap read for every frame).
static WALL_FILTER: std::sync::Mutex<[[f32; 4]; 7]> = std::sync::Mutex::new([[0.0; 4]; 7]);

pub(crate) fn wall_filter() -> [[f32; 4]; 7] {
    WALL_FILTER.lock().map(|f| *f).unwrap_or([[0.0; 4]; 7])
}

/// THE LIVE LEVEL (Max, 2026-10-18: "the slider jumps … make it smooth"):
/// while the Colours slider of the Wallpaper page is dragged, the level under
/// the pointer, as the filter it makes. The box's tiles are graded with it
/// every frame; nothing is saved until the slider is let go.
static WALL_LIVE: std::sync::Mutex<Option<[[f32; 4]; 7]>> = std::sync::Mutex::new(None);

/// Put the dragged level in use (`None`: the saved one is what is shown):
/// the Colours slider's, or (`sat`) the Saturation's (Max, 2026-10-21: "same
/// live on background … so i can turn my BG in to sepia, or even B&W") —
/// the other one as saved.
pub(crate) fn set_wall_live(dragged: Option<(bool, u8)>) {
    let lw = choice().levels_now(Surface::Wall);
    let f = dragged.map(|(sat_dragged, v)| {
        let (l, sat) = if sat_dragged { (lw.tint.unwrap_or(0).min(100), v) } else { (v.min(100), lw.sat) };
        if l == 0 && sat == 50 { [[0.0; 4]; 7] } else { filter_for(&current().id, l, false, sat) }
    });
    if let Ok(mut g) = WALL_LIVE.lock() {
        *g = f;
    }
}

/// Whether a dragged level is in use.
pub(crate) fn wall_live() -> bool {
    WALL_LIVE.lock().is_ok_and(|g| g.is_some())
}

/// The filter the wallpapers' tiles are drawn through now: the dragged
/// level's, else the saved one's.
pub(crate) fn wall_shown() -> [[f32; 4]; 7] {
    WALL_LIVE.lock().ok().and_then(|g| *g).unwrap_or_else(wall_filter)
}

/// Put the wallpapers' filter in use; true when it changed.
fn set_wall_filter(f: [[f32; 4]; 7]) -> bool {
    match WALL_FILTER.lock() {
        Ok(mut g) if *g != f => {
            *g = f;
            true
        }
        _ => false,
    }
}

/// THE FLOORS (the sweep, 2026-10-16): after a tint the hues may sink into
/// the grounds (a light ground tinted toward the accent's own hue left the
/// accent at 2.4:1 on it; Fluor's yellow on white). Each hue is moved in
/// LIGHTNESS only — its hue and chroma kept — until the accent reads 3:1 on
/// the surface and every other hue 2.5:1 on the ground.
fn readable(c: &mut [Rgb; 16], dark: bool, acc: usize) {
    for k in 8..16 {
        let (ground, floor) = if k == acc { (c[1], 3.0) } else { (c[0], 2.5) };
        let (mut l, ch, h) = tint::lch(c[k]);
        let mut x = c[k];
        let mut n = 0;
        while contrast(rel_lum(x), rel_lum(ground)) < floor && n < 60 {
            l = (l + if dark { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
            x = tint::from_lch(l, ch, h);
            n += 1;
        }
        c[k] = x;
    }
}

/// `x` as TEXT on the grounds of `c`: walked in lightness, hue and chroma
/// kept, away from them, until it reads 4.5:1 on base00, base01 and base02.
/// A light look's accent and links stood at 3–4.4:1 as text, 2:1 in full
/// colour; only the text moves (2026-10-20). base02 at 4.5 too: it is where
/// cards, popovers and dialogs carry their words (the legibility audit,
/// 2026-10-23: the accent at 3:1 there failed in 41% of palettes).
pub(crate) fn as_text(c: &[Rgb; 16], x: Rgb) -> Rgb {
    reading(c, x, &[(0, WORDS), (1, WORDS), (2, WORDS)])
}

/// What a program's words read at on its grounds: 4.5:1 with the window in
/// focus, and still 3:1 out of it, under the Dark look's 25% dim
/// (`dim_inactive_for`) — on a near-black ground that takes 4.8.
pub(crate) const WORDS: f32 = 4.8;

/// The words that read ON a fill (a destructive button, a selection): the
/// palette's darkest or brightest, whichever stands further from it — and
/// where neither reads 4.5:1, pure black or white (one of them always does).
pub(crate) fn ink_on_fill(c: &[Rgb; 16], fill: Rgb) -> Rgb {
    let a = rel_lum(fill);
    let (first, last) = (c[0x00], c[0x07]);
    let best = if contrast(a, rel_lum(first)) >= contrast(a, rel_lum(last)) { first } else { last };
    if contrast(a, rel_lum(best)) >= 4.5 {
        return best;
    }
    if contrast(a, 0.0) >= contrast(a, 1.0) { [0, 0, 0] } else { [255, 255, 255] }
}

/// [`Theme::quiet`] for a palette's sixteen.
pub(crate) fn quiet(c: &[Rgb; 16]) -> Rgb {
    as_text(c, c[0x03])
}

/// base04 as words on every ground, base02 included (a status line):
/// 4.5:1 on each.
pub(crate) fn quiet_raised(c: &[Rgb; 16]) -> Rgb {
    reading(c, c[0x04], &[(0, WORDS), (1, WORDS), (2, WORDS)])
}

/// `x` walked in lightness, hue and chroma kept, away from the grounds of `c`
/// until it reads each `(ground, floor)` given.
pub(crate) fn reading(c: &[Rgb; 16], x: Rgb, on: &[(usize, f32)]) -> Rgb {
    let r = |a: Rgb, b: Rgb| contrast(rel_lum(a), rel_lum(b));
    let reads = |y: Rgb| on.iter().all(|&(g, f)| r(y, c[g]) >= f);
    let up = rel_lum(x) > rel_lum(c[1]);
    let (mut l, ch, h) = tint::lch(x);
    let mut y = x;
    let mut n = 0;
    while !reads(y) && n < 100 {
        l = (l + if up { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
        y = tint::from_lch(l, ch, h);
        n += 1;
    }
    y
}

/// Both looks of what the programs wear: the theme's own pair, else the palette's.
fn both(t: &Theme, pal: Option<Palette>) -> ([Rgb; 16], usize, [Rgb; 16], usize) {
    if let Some(p) = t.pair {
        return p;
    }
    match pal {
        Some(p) => (p.dark, p.accent, p.light, p.accent_light),
        None => (t.c, t.accent_slot, t.c, t.accent_slot),
    }
}

/// A palette by id, both its variants: the made one, the stock one, or one
/// of [`all`].
fn palette_of(id: &str) -> Option<Palette> {
    match id {
        DYNAMIC_ID => dynamic_palette(),
        "stock" => parse("stock", STOCK),
        GOLEM => Some(golem()),
        DRAFT_ID => preview(),
        _ => all().into_iter().find(|p| p.id == id),
    }
}

/// Every file of the theme, into `dir`. The lock screen is the shell's and
/// wears the palette always; the rest is the programs'.
pub(crate) fn write_all(dir: &Path, shell: &Theme) {
    let t = &for_apps(shell);
    let put = |name: &str, text: String| crate::persist::write_text("theme", &dir.join(name), &text);
    put("colors.json", colors_json(t, shell, &choice(), &crate::webapps::app_hosts()));
    put("foot.ini", foot_ini(t));
    put("hyprlock.conf", hyprlock_conf(shell));
    put("gtk4.css", gtk4_css(t));
    put("qt.conf", qt_scheme(t));
    put("qt5.conf", qt5_scheme(t));
    put("nvim.lua", nvim_lua(t));
    btop_files(t);
    obs_system_theme();
    for app in ["qt5ct", "qt6ct"] {
        let conf = config_home().join(app).join(format!("{app}.conf"));
        // Ours to write: absent, or already pointing at the theme. One
        // the owner (or a module) set up otherwise is left alone.
        let ours = match std::fs::symlink_metadata(&conf) {
            Err(_) => true,
            Ok(m) if m.file_type().is_symlink() => false,
            Ok(_) => std::fs::read_to_string(&conf).is_ok_and(|s| s.contains("golem/theme/qt")),
        };
        if ours {
            // Qt 5 has one colour role fewer (no Accent): qt5ct takes a scheme
            // of exactly its count and drops any other whole (VLC came up
            // light, 2026-10-15).
            let scheme = if app == "qt5ct" { "qt5.conf" } else { "qt.conf" };
            crate::persist::write_text("theme", &conf, &qtct_conf(&dir.join(scheme), t));
        }
    }
}

/// For anyone who wants the colours (the browser's chrome, scripts): the
/// sixteen, the roles, the accent — the programs' (`t`) — and for Seam's
/// websites, the palette's own ground and ink (`shell`: `web_bg`,
/// `web_ink`), whether they wear it (`web`), and the webapps' hosts, which
/// follow `apps` instead.
fn colors_json(t: &Theme, shell: &Theme, picked: &ColorChoice, webapp_hosts: &[String]) -> String {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), t.name.clone().into());
    m.insert("palette".into(), t.id.clone().into());
    m.insert("dark".into(), t.dark.into());
    m.insert("dynamic".into(), t.dynamic.into());
    // OFF IS NOT LIGHT (Max, 2026-10-16: "the mode off on the sliders
    // should off the colours but not the dark mode … i want dark mode on my
    // apps but not colour"): a slider at Off whose Dark was CLICKED still
    // darkens — neutral greys (the palette walked to 0), no colour. Left to
    // follow the system, Off leaves sites as they are made.
    let apps_dark_off = !t.apps && picked.apps_dark == Some(true);
    let web_dark_off = !t.web && picked.web_dark == Some(true);
    m.insert("apps".into(), (t.apps || apps_dark_off).into());
    m.insert("web".into(), (t.web || web_dark_off).into());
    // THE WEB: its own look and its own slider — the palette's variant in the
    // web look, walked to the Web slider; Seam gives Dark Reader its ground,
    // its ink and the look (`web_dark`).
    let pal = palette_of(&shell.id).unwrap_or_else(golem);
    let wv = if shell.web_dark { pal.dark } else { pal.light };
    // UNDER AMBIENT THE PAGES TAKE THE LOOK, NOT EACH POINT OF THE LIGHT:
    // Seam hands Dark Reader a new pair by restarting it, and every open page
    // loses its colours for that moment — a flash at every step of the day
    // (Max, 2026-10-21: "no jumps at all"). So the pair is the look's as
    // designed; it changes with the look.
    let web_light = if picked.ambient_of(Surface::Web) { 50 } else { shell.web_light };
    // (Off with its Dark clicked: the start of the path — the grounds with no colour.)
    let mut wv = if web_dark_off {
        tint::saturate(&tint::lit(&tint::at(&wv, pal.dark[pal.accent], 0.0, true), true, web_light), shell.web_sat)
    } else {
        tint::walk(&wv, pal.dark[pal.accent], shell.web_dark, shell.web_tint, web_light, shell.web_sat)
    };
    if web_dark_off {
        // …and the ink with no colour either (Hacker's is green: ASUS,
        // 2026-10-16, "the text on wikipedia is green").
        let (l, _, _) = tint::lch(wv[5]);
        wv[5] = tint::from_lch(l, 0.0, 0.0);
    }
    m.insert("web_dark".into(), shell.web_dark.into());
    m.insert("web_bg".into(), format!("#{}", hex(wv[1])).into());
    m.insert("web_ink".into(), format!("#{}", hex(wv[5])).into());
    // The webapps' pair for Dark Reader (the apps' row) — the same: under
    // Ambient the apps' look as designed, not base01/base05, which follow
    // the light for Seam's own bar.
    let steady = if picked.ambient_of(Surface::Apps) { for_apps(&Theme { apps_light: 50, ..shell.clone() }) } else { t.clone() };
    m.insert("apps_bg".into(), format!("#{}", hex(steady.c[1])).into());
    m.insert("apps_ink".into(), format!("#{}", hex(steady.c[5])).into());
    // (What a Seam from before 2026-10-15 reads: it only darkened.)
    m.insert("dark_websites".into(), (t.web && shell.web_dark).into());
    m.insert("icons_tint".into(), shell.icons_tint.into());
    m.insert("icons_dark".into(), shell.icons_dark.into());
    // A webapp kept in its own colours: out of the webapps, and named so
    // Seam leaves its site alone even with the web on.
    let skipped = crate::webapps::hosts_of(picked.skip_in_force());
    m.insert("webapp_hosts".into(), webapp_hosts.iter().filter(|h| !skipped.contains(h)).cloned().collect::<Vec<_>>().into());
    // A webapp with a look of its own (the Apps page's Dark or Light, as a
    // program has): its pages are told that look and, while it wears the
    // palette, given that look's pair — the palette in it, as a program
    // pinned to it wears (`Theme::looks`). Seam: `app_looks` in golem-chrome.js.
    let mut looks = serde_json::Map::new();
    for (id, look) in picked.looks_in_force() {
        let dark = match look.as_str() {
            "dark" => true,
            "light" => false,
            _ => continue,
        };
        for host in crate::webapps::hosts_of(std::slice::from_ref(id)) {
            let mut o = serde_json::Map::new();
            o.insert("dark".into(), dark.into());
            if let (Some((dc, _, lc, _)), false) = (t.looks, skipped.contains(&host)) {
                let c = if dark { dc } else { lc };
                o.insert("bg".into(), format!("#{}", hex(c[1])).into());
                o.insert("ink".into(), format!("#{}", hex(c[5])).into());
            }
            looks.insert(host, o.into());
        }
    }
    m.insert("app_looks".into(), looks.into());
    m.insert("web_skip".into(), skipped.into());
    // The accent, for what Seam's chrome lights with it (the drop on its
    // bar: "your colours on this site").
    m.insert("accent".into(), format!("#{}", hex(t.accent())).into());
    for (i, c) in t.c.iter().enumerate() {
        m.insert(slot_name(i), format!("#{}", hex(*c)).into());
    }
    for (role, c) in [
        ("bg", t.bg()),
        ("surface", t.surface()),
        ("raised", t.raised()),
        ("border", t.border()),
        ("ink_dim", t.ink_dim()),
        // (Seam's muted words: the quiet ink, which reads 4.5:1.)
        ("quiet", t.quiet()),
        ("ink", t.ink()),
        ("accent", t.accent()),
        ("accent_ink", t.accent_ink()),
        ("danger", t.c[0x08]),
        ("warn", t.c[0x0A]),
        ("ok", t.c[0x0B]),
    ] {
        m.insert(role.into(), format!("#{}", hex(c)).into());
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default() + "\n"
}

/// foot: both variants (it switches between them live on a signal, see
/// [`poke`]) and the one to start in.
fn foot_ini(t: &Theme) -> String {
    let pal = palette_of(&t.id);
    let variant = |c: &[Rgb; 16], acc: usize| {
        let accent = as_text(c, c[acc]); // (the links' colour: text)
        let h = |i: usize| hex(c[i]);
        let x = terminal16(c);
        format!(
            "background={bg}\nforeground={fg}\ncursor={bg} {fg}\nselection-background={sel}\nselection-foreground={fg}\n\
             urls={accent}\n\
             regular0={r0}\nregular1={r1}\nregular2={r2}\nregular3={r3}\nregular4={r4}\nregular5={r5}\nregular6={r6}\nregular7={r7}\n\
             bright0={b0}\nbright1={r1}\nbright2={r2}\nbright3={r3}\nbright4={r4}\nbright5={r5}\nbright6={r6}\nbright7={b7}\n",
            bg = h(0),
            fg = h(5),
            sel = h(2),
            accent = hex(accent),
            r0 = hex(x[0]),
            r1 = hex(x[1]),
            r2 = hex(x[2]),
            r3 = hex(x[3]),
            r4 = hex(x[4]),
            r5 = hex(x[5]),
            r6 = hex(x[6]),
            r7 = hex(x[7]),
            b0 = hex(x[8]),
            b7 = hex(x[15]),
        )
    };
    let (dark, da, light, la) = both(t, pal);
    format!(
        "# Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change.\n\
         [main]\ninitial-color-theme={}\n\n[colors-dark]\n{}\n[colors-light]\n{}",
        t.name,
        if t.dark { "dark" } else { "light" },
        variant(&dark, da),
        variant(&light, la),
    )
}

/// hyprlock: variables its config uses (it defines the same ones first, so
/// a missing file still locks — in Golem's own colours).
fn hyprlock_conf(t: &Theme) -> String {
    let rgb = |c: Rgb| format!("rgb({}, {}, {})", c[0], c[1], c[2]);
    format!(
        "# Golem's palette ({}), written by the dock.\n\
         $golem_bg = {}\n$golem_surface = {}\n$golem_ink = {}\n$golem_accent = {}\n$golem_danger = {}\n",
        t.name,
        rgb(t.bg()),
        rgb(t.raised()),
        rgb(t.ink()),
        rgb(t.accent()),
        rgb(t.c[0x08]),
    )
}

/// libadwaita (GTK 4): its colour variables, for BOTH looks — GTK 4.16+
/// reads `prefers-color-scheme`, so a dark/light switch is live in running
/// apps; a palette or accent change reaches them at their next start.
fn gtk4_css(t: &Theme) -> String {
    let pal = palette_of(&t.id);
    let (dark, da, light, la) = both(t, pal);
    let block = |c: &[Rgb; 16], acc: usize| {
        let th = Theme { c: *c, accent_slot: acc, ..t.clone() };
        let h = |x: Rgb| format!("#{}", hex(x));
        let vars = [
            ("accent-bg-color", h(th.accent())),
            ("accent-fg-color", h(th.accent_ink())),
            ("accent-color", h(th.accent_text())),
            ("window-bg-color", h(c[1])),
            ("window-fg-color", h(c[5])),
            ("view-bg-color", h(c[0])),
            ("view-fg-color", h(c[5])),
            ("headerbar-bg-color", h(c[1])),
            ("headerbar-fg-color", h(c[5])),
            ("headerbar-backdrop-color", h(c[1])),
            ("sidebar-bg-color", h(c[1])),
            ("sidebar-fg-color", h(c[5])),
            ("sidebar-backdrop-color", h(c[1])),
            ("secondary-sidebar-bg-color", h(c[0])),
            ("secondary-sidebar-fg-color", h(c[5])),
            ("card-bg-color", h(c[2])),
            ("card-fg-color", h(c[5])),
            ("dialog-bg-color", h(c[2])),
            ("dialog-fg-color", h(c[5])),
            ("popover-bg-color", h(c[2])),
            ("popover-fg-color", h(c[5])),
            ("thumbnail-bg-color", h(c[2])),
            ("thumbnail-fg-color", h(c[5])),
            // (The -colors are words: text-safe. The -fg-colors on the
            // fills are ours too — left out, libadwaita's white sat on a
            // light green, 1.3:1 — the legibility audit, 2026-10-23.)
            ("destructive-bg-color", h(c[8])),
            ("destructive-fg-color", h(ink_on_fill(c, c[8]))),
            ("destructive-color", h(as_text(c, c[8]))),
            ("success-bg-color", h(c[0xB])),
            ("success-fg-color", h(ink_on_fill(c, c[0xB]))),
            ("success-color", h(as_text(c, c[0xB]))),
            ("warning-bg-color", h(c[0xA])),
            ("warning-fg-color", h(ink_on_fill(c, c[0xA]))),
            ("warning-color", h(as_text(c, c[0xA]))),
            ("error-bg-color", h(c[8])),
            ("error-fg-color", h(ink_on_fill(c, c[8]))),
            ("error-color", h(as_text(c, c[8]))),
        ];
        vars.iter().map(|(k, v)| format!("  --{k}: {v};\n")).collect::<String>()
    };
    format!(
        "/* Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change. */\n\
         :root {{\n{}}}\n\n@media (prefers-color-scheme: dark) {{\n  :root {{\n{}  }}\n}}\n",
        t.name,
        block(&light, la),
        block(&dark, da).lines().map(|l| format!("  {l}\n")).collect::<String>(),
    )
}

/// GTK 3, all of it and LIVE (Max, 2026-10-15: "que te sigo diciendo que
/// checkees… el que sigue encontrando gaps soy yo"): stock Adwaita bakes its
/// colours in, but adw-gtk3 — libadwaita's look for GTK 3 — draws with
/// libadwaita's named colours (window_bg_color, accent_bg_color…), which a
/// theme of ours can redefine after importing it. And GTK 3 apps reload
/// their theme when its NAME changes, so the dock writes the palette as a
/// theme under one of two names and switches dconf's gtk-theme between them
/// on every change: open Thunar/Inkscape/Blueman windows follow at once.
/// `base`: adw-gtk3's own stylesheet for the look.
fn gtk3_theme_css(t: &Theme, base: &Path) -> String {
    let c = &t.c;
    let h = |x: Rgb| format!("#{}", hex(x));
    let ink = |x: Rgb| h(ink_for(t, x));
    let mut out = format!(
        "/* Golem's palette ({}) over adw-gtk3, written by the dock. Do not edit: it is rewritten on every change. */\n@import url(\"file://{}\");\n",
        t.name,
        base.display(),
    );
    for (name, col) in [
        ("window_bg_color", h(c[1])),
        ("window_fg_color", h(c[5])),
        ("view_bg_color", h(c[0])),
        ("view_fg_color", h(c[5])),
        ("headerbar_bg_color", h(c[1])),
        ("headerbar_fg_color", h(c[5])),
        ("headerbar_backdrop_color", h(c[1])),
        ("sidebar_bg_color", h(c[1])),
        ("sidebar_fg_color", h(c[5])),
        ("sidebar_backdrop_color", h(c[1])),
        ("card_bg_color", h(c[2])),
        ("card_fg_color", h(c[5])),
        ("dialog_bg_color", h(c[2])),
        ("dialog_fg_color", h(c[5])),
        ("popover_bg_color", h(c[2])),
        ("popover_fg_color", h(c[5])),
        ("thumbnail_bg_color", h(c[2])),
        ("thumbnail_fg_color", h(c[5])),
        ("accent_color", h(t.accent_text())),
        ("accent_bg_color", h(t.accent())),
        ("accent_fg_color", h(t.accent_ink())),
        // GTK 3 reads its SELECTED colours from these, and adw-gtk3 names them
        // after the accent at its own parse — so they kept its white text over
        // a palette's light accent (Seam's chosen suggestion white on white —
        // Max, 2026-10-20, India Ink). Said outright here.
        ("theme_selected_bg_color", h(t.accent())),
        ("theme_selected_fg_color", h(t.accent_ink())),
        // (The `_color`s are WORDS — an error's label, a destructive
        // flat button — so the text-safe colour; the `_bg_color`s fills.)
        ("destructive_color", h(t.as_text(c[8]))),
        ("destructive_bg_color", h(c[8])),
        ("destructive_fg_color", ink(c[8])),
        ("error_color", h(t.as_text(c[8]))),
        ("error_bg_color", h(c[8])),
        ("error_fg_color", ink(c[8])),
        ("success_color", h(t.as_text(c[0xB]))),
        ("success_bg_color", h(c[0xB])),
        ("success_fg_color", ink(c[0xB])),
        ("warning_color", h(t.as_text(c[0xA]))),
        ("warning_bg_color", h(c[0xA])),
        ("warning_fg_color", ink(c[0xA])),
    ] {
        out.push_str(&format!("@define-color {name} {col};\n"));
    }
    out
}

/// The palette's darkest or brightest, whichever reads on `bg` (else pure
/// black or white: [`ink_on_fill`]).
fn ink_for(t: &Theme, bg: Rgb) -> Rgb {
    ink_on_fill(&t.c, bg)
}

/// adw-gtk3's stylesheet for a look, where the system keeps it.
pub(crate) fn adw_gtk3(dark: bool) -> Option<PathBuf> {
    let name = if dark { "adw-gtk3-dark" } else { "adw-gtk3" };
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    std::iter::once(data_home)
        .chain(dirs.split(':').map(str::to_owned))
        .map(|d| PathBuf::from(d).join("themes").join(name).join("gtk-3.0").join("gtk.css"))
        .find(|p| p.is_file())
}

/// The two names the GTK 3 theme takes in turn.
const GTK3_NAMES: [&str; 2] = ["Golem-A", "Golem-B"];

/// Write the GTK 3 theme under the name NOT in use and switch to it.
fn gtk3_live(t: &Theme) {
    let Some(base) = adw_gtk3(t.dark) else {
        return; // No adw-gtk3: Adwaita, set by the Dark mode setting.
    };
    let now = out("dconf", &["read", "/org/gnome/desktop/interface/gtk-theme"])
        .map(|o| o.trim().trim_matches('\'').to_owned())
        .unwrap_or_default();
    let next = if now == GTK3_NAMES[0] { GTK3_NAMES[1] } else { GTK3_NAMES[0] };
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let dir = PathBuf::from(&data_home).join("themes").join(next);
    crate::persist::write_text("theme", &dir.join("gtk-3.0").join("gtk.css"), &gtk3_theme_css(t, &base));
    // The theme's GTK 4 half: GTK 4 apps take the theme NAME from the same
    // setting (at their start, and live), and a theme without a gtk-4.0
    // stylesheet makes them fall back to GTK's whole Default theme — under
    // libadwaita's own, which replaces it with an EMPTY one on purpose. So
    // this half holds only the palette's variables (the user stylesheet's
    // own text): libadwaita apps stay clean, and the colours are there for
    // a GTK 4 app without libadwaita.
    crate::persist::write_text("theme", &dir.join("gtk-4.0").join("gtk.css"), &gtk4_css(t));
    crate::persist::write_text(
        "theme",
        &dir.join("index.theme"),
        &format!("[Desktop Entry]\nType=X-GNOME-Metatheme\nName={next}\nComment=Golem's palette over adw-gtk3, written by the dock\n\n[X-GNOME-Metatheme]\nGtkTheme={next}\n"),
    );
    run("dconf", &["write", "/org/gnome/desktop/interface/gtk-theme", &format!("'{next}'")]);
    // THE PINNED ONES (an app's own Dark or Light): the palette in each look
    // under a fixed name, written with the one in use, read at an app's start.
    if let Some((dc, da, lc, la)) = t.looks.or(t.pair) {
        for (name, dark, c, acc) in [(GTK3_PINNED[0], true, dc, da), (GTK3_PINNED[1], false, lc, la)] {
            let Some(base) = adw_gtk3(dark) else { continue };
            let v = Theme { c, accent_slot: acc, dark, ..t.clone() };
            let dir = PathBuf::from(&data_home).join("themes").join(name);
            crate::persist::write_text("theme", &dir.join("gtk-3.0").join("gtk.css"), &gtk3_theme_css(&v, &base));
            crate::persist::write_text("theme", &dir.join("gtk-4.0").join("gtk.css"), &gtk4_css(&v));
            crate::persist::write_text("theme", &dir.join("index.theme"), &format!("[Desktop Entry]\nType=X-GNOME-Metatheme\nName={name}\nComment=Golem's palette, pinned to a look, written by the dock\n\n[X-GNOME-Metatheme]\nGtkTheme={name}\n"));
        }
    }
}

/// Qt (through qt5ct/qt6ct): a colour scheme, one colour per palette role
/// in `QPalette::ColorRole` order.
fn qt_scheme(t: &Theme) -> String {
    let c = &t.c;
    let a = |x: Rgb| format!("#ff{}", hex(x));
    let dim = |x: Rgb| format!("#80{}", hex(x));
    // (Disabled words: the palette's dim ink, which reads 3:1 on every
    // ground — the ink at half its alpha did not, on 47% of palettes.)
    let _ = dim;
    let roles = |text: Rgb, faint: bool| {
        let ink = if faint { a(t.ink_dim()) } else { a(text) };
        [
            ink.clone(),          // WindowText
            a(c[2]),              // Button
            a(c[3]),              // Light
            a(c[2]),              // Midlight
            a(c[0]),              // Dark
            a(c[1]),              // Mid
            ink.clone(),          // Text
            a(c[7]),              // BrightText
            ink,                  // ButtonText
            a(c[0]),              // Base
            a(c[1]),              // Window
            "#ff000000".into(),   // Shadow
            a(t.accent()),        // Highlight
            a(t.accent_ink()),    // HighlightedText
            a(t.as_text(c[0xD])), // Link
            a(t.as_text(c[0xE])), // LinkVisited
            a(c[1]),              // AlternateBase
            a(c[0]),              // NoRole
            a(c[2]),              // ToolTipBase
            a(c[5]),              // ToolTipText
            a(c[4]),              // PlaceholderText
            a(t.accent()),        // Accent
        ]
        .join(", ")
    };
    format!(
        "[ColorScheme]\nactive_colors={}\ndisabled_colors={}\ninactive_colors={}\n",
        roles(t.ink(), false),
        roles(t.ink(), true),
        roles(t.ink(), false),
    )
}

/// [`qt_scheme`] for Qt 5: its 21 roles (Qt 6 added Accent, the last).
fn qt5_scheme(t: &Theme) -> String {
    qt_scheme(t)
        .lines()
        .map(|l| match l.split_once('=') {
            Some((k, v)) if k.ends_with("_colors") => {
                let mut roles: Vec<&str> = v.split(", ").collect();
                roles.truncate(21);
                format!("{k}={}", roles.join(", "))
            }
            _ => l.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// What a program needs to wear the palette through Qt — qt5ct/qt6ct as the
/// platform theme, and where the system keeps Qt's plugins — when the
/// session does not carry it yet: NixOS puts both in the login profile, so a
/// session older than Golem's Qt theming has neither (VLC stayed light,
/// 2026-10-15). Given to every program the dock starts and to the
/// compositor's own ([`App::colors_apply`]); an owner's own choice of
/// platform theme is kept.
pub(crate) fn launch_vars() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let user = std::env::var("USER").unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    let roots = [
        PathBuf::from("/run/current-system/sw/lib"),
        PathBuf::from(format!("/etc/profiles/per-user/{user}/lib")),
        PathBuf::from(format!("{home}/.nix-profile/lib")),
    ];
    let mut dirs: Vec<String> = std::env::var("QT_PLUGIN_PATH")
        .map(|v| v.split(':').filter(|s| !s.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    let mut have_qtct = false;
    for root in &roots {
        let Ok(read) = std::fs::read_dir(root) else {
            continue;
        };
        let mut found: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("qt-")))
            .map(|p| p.join("plugins"))
            .filter(|p| p.is_dir())
            .collect();
        found.sort();
        for p in found {
            let themes = p.join("platformthemes");
            have_qtct |= themes.join("libqt5ct.so").exists() || themes.join("libqt6ct.so").exists();
            let s = p.to_string_lossy().into_owned();
            if !dirs.contains(&s) {
                dirs.push(s);
            }
        }
    }
    if !have_qtct {
        return out;
    }
    if std::env::var("QT_QPA_PLATFORMTHEME").is_err() {
        out.push(("QT_QPA_PLATFORMTHEME", "qt5ct".to_owned()));
    }
    out.push(("QT_PLUGIN_PATH", dirs.join(":")));
    out
}

fn qtct_conf(scheme: &Path, t: &Theme) -> String {
    // (The colours' stamp: qt5ct/qt6ct reload their palette LIVE when their
    // own folder changes, and this file must differ for that — the scheme
    // file it points at lives elsewhere. Max, 2026-10-17: KDE Connect did
    // not change live.)
    let stamp = qt_scheme(t).bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    format!(
        "[Appearance]\ncolor_scheme_path={}\ncustom_palette=true\nicon_theme={}\nstandard_dialogs=default\nstyle=Fusion\n\n[Golem]\nstamp={stamp:016x}\n",
        scheme.display(),
        qt_icon_theme(t.dark),
    )
}

/// The icon theme Qt apps are told: the apps' tinted one while it is in use
/// (the one dconf names), else Golem's own in the look.
fn qt_icon_theme(dark: bool) -> String {
    let named = std::process::Command::new("dconf")
        .args(["read", "/org/gnome/desktop/interface/icon-theme"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_owned())
        .unwrap_or_default();
    if named.starts_with("Golem-Tinted-") {
        return named;
    }
    if dark { "Golem-Dark" } else { "Golem" }.to_owned()
}

/// btop (Omarchy themes it too): a `golem` theme in btop's own themes folder,
/// and btop pointed at it while it still wears its out-of-the-box theme —
/// a theme the owner picked in btop is theirs and left alone.
fn btop_files(t: &Theme) {
    // Only where btop is: a theme for a program that is not there is clutter
    // in the owner's config (the laptops ship without it).
    if !crate::launch::on_path("btop") && !config_home().join("btop").is_dir() {
        return;
    }
    let dir = config_home().join("btop");
    crate::persist::write_text("theme", &dir.join("themes").join("golem.theme"), &btop_theme(t));
    let conf = dir.join("btop.conf");
    match std::fs::read_to_string(&conf) {
        Ok(text) => {
            let mut changed = false;
            let lines: Vec<String> = text
                .lines()
                .map(|l| {
                    let v = l.trim_start().strip_prefix("color_theme").map(|r| r.trim_start().trim_start_matches('=').trim().trim_matches('"'));
                    match v {
                        Some("Default") | Some("") => {
                            changed = true;
                            "color_theme = \"golem\"".to_owned()
                        }
                        _ => l.to_owned(),
                    }
                })
                .collect();
            if changed {
                crate::persist::write_text("theme", &conf, &(lines.join("\n") + "\n"));
            }
        }
        // Never started: it takes the rest of its settings from its defaults.
        Err(_) => crate::persist::write_text("theme", &conf, "color_theme = \"golem\"\n"),
    }
}

/// OBS draws with its own themes (Yami, its default, is its own grey) — but
/// its "System" theme draws with Qt's palette, which is ours (Max,
/// 2026-10-15: "y porque obs no cambia?"). Put on it while OBS has no theme
/// of the owner's choosing, and never while it runs: it writes its config
/// back when it closes.
fn obs_system_theme() {
    let ini = config_home().join("obs-studio").join("user.ini");
    let Ok(text) = std::fs::read_to_string(&ini) else {
        return; // Never opened: Golem's OBS fit seeds it with this theme.
    };
    if running(&["obs", ".obs-wrapped"]) {
        return;
    }
    if let Some(out) = with_obs_theme(&text) {
        crate::persist::write_text("theme", &ini, &out);
    }
}

/// `user.ini` with the System theme under `[Appearance]`, or `None` when it
/// already names a theme.
fn with_obs_theme(text: &str) -> Option<String> {
    const THEME: &str = "Theme=com.obsproject.System";
    let mut in_appearance = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_appearance = l == "[Appearance]";
        } else if in_appearance && l.starts_with("Theme=") {
            return None;
        }
    }
    let mut out = String::new();
    let mut done = false;
    for l in text.lines() {
        out.push_str(l);
        out.push('\n');
        if !done && l.trim() == "[Appearance]" {
            out.push_str(THEME);
            out.push('\n');
            done = true;
        }
    }
    if !done {
        if !out.ends_with("\n\n") && !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[Appearance]\n");
        out.push_str(THEME);
        out.push('\n');
    }
    Some(out)
}

/// Whether a process of ours by one of these names is running.
fn running(names: &[&str]) -> bool {
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
        std::fs::read_to_string(e.path().join("comm")).is_ok_and(|n| names.contains(&n.trim()))
            && std::fs::read_to_string(e.path().join("status")).ok().is_some_and(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("Uid:"))
                    .and_then(|v| v.split_whitespace().next()?.parse::<u32>().ok())
                    == Some(me)
            })
    })
}

fn btop_theme(t: &Theme) -> String {
    let c = &t.c;
    let h = |i: usize| format!("#{}", hex(c[i]));
    let a = format!("#{}", hex(t.accent_text()));
    let mut out = format!("# Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change.\n", t.name);
    let mut put = |k: &str, v: &str| out.push_str(&format!("theme[{k}]=\"{v}\"\n"));
    put("main_bg", &h(0));
    put("main_fg", &h(5));
    put("title", &a);
    put("hi_fg", &a);
    put("selected_bg", &h(2));
    put("selected_fg", &h(7));
    put("inactive_fg", &format!("#{}", hex(t.quiet())));
    put("graph_text", &h(4));
    put("meter_bg", &h(2));
    put("proc_misc", &a);
    for k in ["cpu_box", "mem_box", "net_box", "proc_box", "div_line"] {
        put(k, &h(3));
    }
    // Graphs run through the palette's hues: cool to hot, green to blue.
    for (k, (s, m, e)) in [
        ("temp", (0xD, 0xA, 0x8)),
        ("cpu", (0xB, 0xA, 0x8)),
        ("free", (0xB, 0xC, 0xD)),
        ("cached", (0xD, 0xC, 0xB)),
        ("available", (0xC, 0xB, 0xA)),
        ("used", (0xA, 0x9, 0x8)),
        ("download", (0xC, 0xD, 0xE)),
        ("upload", (0xE, 0x9, 0x8)),
        ("process", (0xB, 0xA, 0x8)),
    ] {
        // (Graphs are shapes: 3:1 on the ground and on the meters' base02.)
        let g = |i: usize| format!("#{}", hex(reading(c, c[i], &[(0, 3.0), (2, 3.0)])));
        put(&format!("{k}_start"), &g(s));
        put(&format!("{k}_mid"), &g(m));
        put(&format!("{k}_end"), &g(e));
    }
    out
}

/// Neovim (mini.base16, Golem's home layer loads this at each start).
fn nvim_lua(t: &Theme) -> String {
    // (mini.base16 reads base03 as comments and line numbers, base04 as the
    // status line's words: Golem's are grounds — given as words here.)
    let mut c = t.c;
    c[0x03] = quiet(&t.c);
    c[0x04] = quiet_raised(&t.c);
    // (…and the hues, its syntax and its messages, as words.)
    for (slot, hue) in c.iter_mut().zip(t.c).skip(8) {
        *slot = as_text(&t.c, hue);
    }
    let palette: String = c.iter().enumerate().map(|(i, c)| format!("    {} = '#{}',\n", slot_name(i), hex(*c))).collect();
    format!(
        "-- Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change.\n\
         vim.o.background = '{}'\n\
         require('mini.base16').setup({{\n  palette = {{\n{}  }},\n  use_cterm = true,\n}})\n\
         vim.g.colors_name = 'golem'\n\
         -- (mini.base16's WildMenu is red on yellow: the search's pair instead.)\n\
         vim.api.nvim_set_hl(0, 'WildMenu', {{ fg = '#{}', bg = '#{}' }})\n",
        t.name,
        if t.dark { "dark" } else { "light" },
        palette,
        hex(c[1]),
        hex(c[0xA]),
    )
}

/// libadwaita's named accent nearest the palette's (apps that ask the
/// portal for the accent get a name, not a colour).
fn adwaita_accent(c: Rgb) -> &'static str {
    let f = |v: u8| f32::from(v) / 255.0;
    let (h, s, _) = crate::options::rgb_to_hsl(f(c[0]), f(c[1]), f(c[2]));
    if s < 0.15 {
        return "slate";
    }
    let deg = h * 360.0;
    match deg {
        d if !(15.0..345.0).contains(&d) => "red",
        d if d < 45.0 => "orange",
        d if d < 70.0 => "yellow",
        d if d < 160.0 => "green",
        d if d < 195.0 => "teal",
        d if d < 255.0 => "blue",
        d if d < 300.0 => "purple",
        _ => "pink",
    }
}

/// Tell the running apps.
fn poke(t: &Theme, look_changed: bool) {
    let accent = format!("'{}'", adwaita_accent(t.accent()));
    run("dconf", &["write", "/org/gnome/desktop/interface/accent-color", &accent]);
    // Every open terminal, at once (a palette, a look): foot reads its
    // config only at its start, so its windows are told the colours.
    let _ = look_changed;
    foot_live(t);
    gtk3_live(t);
    sourceview_live(t);
    // btop reads its config again on SIGUSR2 (and with it, its theme).
    signal_all("btop", libc::SIGUSR2);
}

/// GtkSourceView (GNOME Text Editor's writing area, Builder…): a colour
/// scheme of the palette — the area does not take the GTK theme's colours.
/// ONE name, rewritten: an app reads its schemes once, at its start (the
/// ASUS, 2026-10-16: switching between two names to make it re-read showed
/// a PREVIOUS palette's copy — Jade's green under Braun), so a change
/// reaches an editor at its next start, as GTK 4's colours do. An owner who
/// picked another scheme in the editor keeps it.
fn sourceview_live(t: &Theme) {
    const NAME: &str = "golem";
    let key = "/org/gnome/TextEditor/style-scheme";
    let now = out("dconf", &["read", key]).map(|o| o.trim().trim_matches('\'').to_owned()).unwrap_or_default();
    let ours = now.is_empty() || now == NAME || now == "golem-a" || now == "golem-b";
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let dir = PathBuf::from(data_home).join("gtksourceview-5").join("styles");
    crate::persist::write_text("theme", &dir.join(format!("{NAME}.xml")), &sourceview_scheme(t, NAME));
    // (The two names of the first cut go.)
    for old in ["golem-a", "golem-b"] {
        let _ = std::fs::remove_file(dir.join(format!("{old}.xml")));
    }
    if ours && now != NAME {
        run("dconf", &["write", key, &format!("'{NAME}'")]);
    }
}

fn sourceview_scheme(t: &Theme, id: &str) -> String {
    let c = &t.c;
    let h = |x: Rgb| format!("#{}", hex(x));
    let styles = [
        ("text", format!(r#"foreground="{}" background="{}""#, h(c[5]), h(c[0]))),
        ("selection", format!(r#"foreground="{}" background="{}""#, h(c[6]), h(c[2]))),
        ("current-line", format!(r#"background="{}""#, h(c[1]))),
        ("line-numbers", format!(r#"foreground="{}" background="{}""#, h(t.quiet()), h(c[0]))),
        ("current-line-number", format!(r#"foreground="{}" background="{}""#, h(c[5]), h(c[1]))),
        ("cursor", format!(r#"foreground="{}""#, h(t.accent()))),
        ("bracket-match", format!(r#"foreground="{}" background="{}" bold="true""#, h(t.accent_text()), h(c[2]))),
        ("search-match", format!(r#"foreground="{}" background="{}""#, h(ink_on_fill(c, c[3])), h(c[3]))),
        ("def:comment", format!(r#"foreground="{}" italic="true""#, h(t.quiet()))),
        ("def:keyword", format!(r#"foreground="{}" bold="true""#, h(t.as_text(c[0xE])))),
        ("def:statement", format!(r#"foreground="{}""#, h(t.as_text(c[0xE])))),
        ("def:type", format!(r#"foreground="{}""#, h(t.as_text(c[0xA])))),
        ("def:string", format!(r#"foreground="{}""#, h(t.as_text(c[0xB])))),
        ("def:special-char", format!(r#"foreground="{}""#, h(t.as_text(c[0xC])))),
        ("def:constant", format!(r#"foreground="{}""#, h(t.as_text(c[0x9])))),
        ("def:number", format!(r#"foreground="{}""#, h(t.as_text(c[0x9])))),
        ("def:identifier", format!(r#"foreground="{}""#, h(t.as_text(c[0xD])))),
        ("def:function", format!(r#"foreground="{}""#, h(t.as_text(c[0xD])))),
        ("def:preprocessor", format!(r#"foreground="{}""#, h(t.as_text(c[0xC])))),
        ("def:error", format!(r#"foreground="{}" underline="error""#, h(t.as_text(c[8])))),
        ("def:note", format!(r#"foreground="{}" bold="true""#, h(t.accent_text()))),
        ("def:heading", format!(r#"foreground="{}" bold="true""#, h(t.accent_text()))),
        ("def:link-text", format!(r#"foreground="{}" underline="single""#, h(t.as_text(c[0xD])))),
    ];
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change. -->\n<style-scheme id=\"{id}\" name=\"Golem\" version=\"1.0\">\n  <author>Golem</author>\n  <description>The palette in use</description>\n",
        t.name.replace(['<', '>', '&', '"'], "")
    );
    for (name, attrs) in styles {
        out.push_str(&format!("  <style name=\"{name}\" {attrs}/>\n"));
    }
    out.push_str("</style-scheme>\n");
    out
}

/// Send `sig` to every process of ours named `comm`.
fn signal_all(comm: &str, sig: libc::c_int) {
    if rig() {
        return;
    }
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        if !std::fs::read_to_string(e.path().join("comm")).is_ok_and(|n| n.trim() == comm) {
            continue;
        }
        let owner = std::fs::read_to_string(e.path().join("status")).ok().and_then(|s| {
            s.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().next()?.parse::<u32>().ok())
        });
        if owner == Some(me) {
            // SAFETY: a plain kill(2) of a process of our own user.
            unsafe { libc::kill(pid, sig) };
        }
    }
}

/// THE OPEN TERMINALS FOLLOW, LIVE (Max, 2026-10-15: "we need the themes to
/// apply live… that is how it works on omarchy"). foot never reads its
/// config again, but it takes colour sequences from what runs in it: OSC 4
/// (the sixteen), 10/11/12 (ink, ground, cursor), 17/19 (selection) —
/// the way pywal recolours terminals. They are written into the terminal
/// device of every program foot started (its shell): the terminal reads
/// them as output and changes on the spot, and so does whatever draws with
/// its colours (yazi, bat, ls, git, the prompt). The order is foot.ini's
/// (`foot_ini`), so a new window and an old one agree.
/// THE TERMINAL'S SIXTEEN, by meaning in both looks: black the raised
/// ground, white the ink, bright black the quiet words ([`Theme::quiet`];
/// base03, a ground, was invisible as the dim text programs print with it).
/// One list for foot's file and for the live OSC, so the two never part.
fn terminal16(c: &[Rgb; 16]) -> [Rgb; 16] {
    // (The hues are words in a terminal: text-safe, 4.5:1 on its grounds.)
    let w = |i: usize| as_text(c, c[i]);
    [c[2], w(8), w(0xB), w(0xA), w(0xD), w(0xE), w(0xC), c[5], quiet(c), w(8), w(0xB), w(0xA), w(0xD), w(0xE), w(0xC), c[7]]
}

fn foot_osc(t: &Theme) -> String {
    let c = &t.c;
    let sixteen = terminal16(c);
    let mut out = String::new();
    for (i, col) in sixteen.iter().enumerate() {
        out.push_str(&format!("\x1b]4;{i};#{}\x1b\\", hex(*col)));
    }
    for (code, col) in [(10, c[5]), (11, c[0]), (12, c[5]), (17, c[2]), (19, c[5])] {
        out.push_str(&format!("\x1b]{code};#{}\x1b\\", hex(col)));
    }
    out
}

fn foot_live(t: &Theme) {
    if rig() {
        return;
    }
    let seq = foot_osc(t);
    for pty in foot_ptys() {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let wrote = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(&pty)
            .and_then(|mut f| f.write_all(seq.as_bytes()));
        if let Err(e) = wrote {
            debug!("palette: {}: {e}", pty.display());
        }
    }
}

/// The terminal devices of the programs every foot of ours started.
fn foot_ptys() -> Vec<PathBuf> {
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    let procs: Vec<(i32, i32, String, Option<u32>)> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pid = e.file_name().to_str()?.parse::<i32>().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            // "pid (comm) state ppid …" — comm may hold spaces and parens.
            let close = stat.rfind(')')?;
            let comm = stat.get(stat.find('(')? + 1..close)?.to_owned();
            let ppid = stat[close + 1..].split_whitespace().nth(1)?.parse::<i32>().ok()?;
            let uid = std::fs::read_to_string(e.path().join("status")).ok().and_then(|s| {
                s.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().next()?.parse::<u32>().ok())
            });
            Some((pid, ppid, comm, uid))
        })
        .collect();
    let feet: std::collections::HashSet<i32> =
        procs.iter().filter(|(_, _, comm, uid)| comm == "foot" && *uid == Some(me)).map(|(pid, ..)| *pid).collect();
    let mut out: Vec<PathBuf> = procs
        .iter()
        .filter(|(_, ppid, _, uid)| feet.contains(ppid) && *uid == Some(me))
        .filter_map(|(pid, ..)| std::fs::read_link(format!("/proc/{pid}/fd/0")).ok())
        .filter(|p| p.starts_with("/dev/pts/"))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Run a program for its effect, or end it at [`PATIENCE`] (a bus that
/// never answers must not hold the palette thread, and with it every later
/// change, for good).
fn run(program: &str, args: &[&str]) {
    if rig() {
        debug!("palette: a nested test dock leaves {program} alone");
        return;
    }
    if out(program, args).is_none() {
        debug!("palette: {program} {args:?} did not succeed");
    }
}

/// A NESTED TEST DOCK (`GOLEM_RIG`, the rig's: ~/.cache/golem-nested): it
/// runs as the owner, so it must not tell the owner's live session anything
/// — no dconf, no terminal's colours, no signal, no systemd environment. It
/// writes its files in its own XDG dirs and draws; nothing else.
pub(crate) fn rig() -> bool {
    std::env::var_os("GOLEM_RIG").is_some()
}

/// A program's standard output, or `None` when it is missing, failed, or
/// was ended at [`PATIENCE`].
fn out(program: &str, args: &[&str]) -> Option<String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
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
            Ok(None) if start.elapsed() < PATIENCE => std::thread::sleep(std::time::Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    status.filter(|s| s.success()).map(|_| text)
}

/// The longest a program asked for a reading or a change may take.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(4);


// --------------------------------------------------------------- the shell

impl crate::App {
    /// Read the colour choice again and put it in use: the shell redraws in
    /// it and the apps get it. At start, on a pick in the panel, and when
    /// the look turns dark or light.
    pub(crate) fn colors_apply(&mut self, look_changed: bool) {
        // Programs the compositor starts (keys, a terminal's children) get
        // Qt's palette route too, without a new login.
        let vars = launch_vars();
        for (k, v) in &vars {
            // (Quoted for Lua: a ' or a \ in a path must not end the string.)
            let v = v.replace('\\', "\\\\").replace('\'', "\\'");
            crate::hypr::eval(&format!("hl.env('{k}', '{v}')"));
        }
        // …and so do the apps systemd and D-Bus start (Easy Effects runs as
        // a user service; KDE Connect's daemon is D-Bus activated) — once a
        // dock start, off the loop.
        static TOLD: std::sync::Once = std::sync::Once::new();
        TOLD.call_once(|| {
            let pairs: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
            if pairs.is_empty() {
                return;
            }
            let _ = std::thread::Builder::new().name("qt-env".into()).spawn(move || {
                if rig() {
                return;
            }
            let _ = std::process::Command::new("systemctl").args(["--user", "set-environment"]).args(&pairs).status();
                let _ = std::process::Command::new("dbus-update-activation-environment").arg("--systemd").args(&pairs).status();
            });
        });
        let mut picked = choice();
        if previewing() {
            picked.palette = Some(DRAFT_ID.to_owned());
        }
        // (A fade of the Lighting in progress is what is shown: whatever else
        // asks for a re-apply must not jump it back — `lighting.rs`.)
        let lit_now = picked.levels_now(Surface::Shell).light;
        tint::set_lighting(lit_now);
        crate::lighting::set_veil(picked.ambient_of(Surface::Wall));
        crate::lighting::tell_overview();
        self.lighting_mirror(picked.ambient_of(Surface::Shell) || picked.ambient_of(Surface::Wall));
        let theme = resolve(&picked, crate::look::dark());
        // The Dynamic palette of a new wallpaper is being made: Golem's
        // stands in, and this comes back when it is ready.
        if theme.dynamic && dynamic_pending() {
            let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(250));
            let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut crate::App| {
                app.colors_apply(look_changed);
                calloop::timer::TimeoutAction::Drop
            });
        }
        set_icon_filter(&theme);
        // The wallpapers' slider: the box's thumbnails and the picture up.
        let lw = picked.levels_now(Surface::Wall);
        let wall = filter_for(&theme.id, lw.tint.unwrap_or(0).min(100), false, lw.sat);
        let wall = if wall[0][0] > 0.0 || lw.sat != 50 { wall } else { [[0.0; 4]; 7] };
        if set_wall_filter(wall) {
            crate::wall_tint::refresh();
        }
        // (The Shell's own Palette and Saturation move only its pair: the
        // theme the apps wear stays as it was, yet every shell surface wears it.)
        let shell_before = shell();
        set_shell(&picked, crate::look::dark());
        let shell_moved = shell() != shell_before;
        let c = choice();
        set_skipped_programs(&self.entries, c.skip_in_force());
        set_looked_programs(&self.entries, c.looks_in_force());
        // The Shell's and the dock icons' Transparency, for every frame (the
        // reduce-transparency intent: none — its glass is solid already).
        let reduce = self.config.accessibility.reduce_transparency;
        let clear_moved = crate::options::set_clear(
            if reduce { 0 } else { c.clear_of(Surface::Shell) },
            if reduce { 0 } else { c.clear_of(Surface::Icons) },
        );
        // The windows' opacity is the compositor's: told when it changes — a
        // pick, Auto, a start (settings.lua, written with the choice, has it
        // for the compositor's own start).
        let rules = clear_rules_lua(&c);
        if let Ok(mut told) = RULES_TOLD.lock() {
            if *told != rules {
                crate::hypr::eval(&rules);
                *told = rules;
            }
        }
        let changed = set(theme);
        // A move of the Lighting alone leaves the theme as it was, yet the
        // shell, the apps and the web all wear it.
        let lit_moved = crate::lighting::applied(lit_now);
        if changed || look_changed || lit_moved {
            publish(current(), look_changed);
        }
        // The wash over the wallpaper, wherever its light now puts it (the
        // wallpaper's own Lighting slider moved it only at the next start).
        self.veil_follow();
        if changed || lit_moved || shell_moved {
            self.reassert_window_border();
            self.draw_options();
            self.draw();
            self.filer_redraw_all();
            // (The desktop's icons wear the same filter: Max, 2026-10-17,
            // "the icons on the desktop don't change live".)
            self.request_desktop_draw();
        }
        // The Shell's Transparency: every surface of the shell drawn again.
        if clear_moved {
            self.draw_options();
            self.draw();
            self.draw_card();
            self.request_desktop_draw();
            self.request_desktop_top_draw();
        }
    }

    /// A choice in the "Colours" setting: save it, then put it in use.
    pub(crate) fn colors_pick(&mut self, key: &str, value: &str) {
        // The slider is the owner's own hand, and Auto turned off keeps the
        // light it was showing: a fade in progress ends here (`lighting.rs`).
        let frozen = if matches!(key, "colors-lighting" | "colors-lighting-auto" | "colors-ambient" | "colors-wall-ambient" | "colors-auto" | "colors-wall-auto") { self.lighting_cancel() } else { None };
        // (What the sun gave while Ambient was on — the fade's step, else its
        // last line: the look and the light the sliders take over from, so
        // nothing jumps.)
        let frozen = frozen.map(line_to).or_else(crate::lighting::ambient_line);
        GolemSettings::change(|s| {
            let mut c = s.colors.clone().unwrap_or_default();
            match key {
                "colors-palette" => {
                    c.palette = Some(value.to_owned());
                    c.dynamic = None;
                    // Dynamic is made FROM the wallpaper, so the wallpaper
                    // stops being made from the colours; the last picture stays.
                    crate::wall_follow::after_palette(s, value);
                }
                // A slider: "0".."100", "made" (the palette as made); the old "on"/"off" too.
                "colors-apps" | "colors-web" | "colors-icons" | "colors-shell" => {
                    let level = match value {
                        "made" | "on" => None,
                        "off" => Some(0),
                        v => match v.parse::<u8>() {
                            Ok(n) => Some(n.min(100)),
                            Err(_) => return,
                        },
                    };
                    c = c.settled();
                    match key {
                        "colors-apps" => c.apps_tint = level,
                        "colors-web" => c.web_tint = level,
                        "colors-shell" => c.shell_tint = level,
                        _ => c.icons_tint = level.unwrap_or(50),
                    }
                }
                // The old general Lighting: every surface's; manual from then on.
                "colors-lighting" => match value.parse::<u8>() {
                    Ok(n) => {
                        for own in [&mut c.shell_light, &mut c.apps_light, &mut c.icons_light, &mut c.web_light] {
                            *own = Some(n.min(100));
                        }
                        c.ambient = None;
                    }
                    Err(_) => return,
                },
                // Ambient (the old Auto lighting): off keeps the light it showed.
                "colors-lighting-auto" | "colors-ambient" => match value {
                    "on" => c.ambient = Some(true),
                    "off" => {
                        c.ambient = None;
                        freeze(&mut c, &[Surface::Shell, Surface::Apps, Surface::Icons, Surface::Web], frozen);
                    }
                    _ => return,
                },
                "colors-wall-ambient" => match value {
                    "on" => c.wall_ambient = Some(true),
                    "off" => {
                        c.wall_ambient = None;
                        freeze(&mut c, &[Surface::Wall], frozen);
                    }
                    _ => return,
                },
                // AUTO per tab: Golem's defaults + Ambient; off leaves the sliders
                // where Auto had them (nothing jumps).
                "colors-auto" | "colors-wall-auto" => {
                    let wall = key == "colors-wall-auto";
                    let on = match value {
                        "on" => true,
                        "off" => false,
                        _ => return,
                    };
                    if wall {
                        c.wall_auto = on.then_some(true);
                        c.wall_ambient = on.then_some(true);
                    } else {
                        c.auto = on.then_some(true);
                        c.ambient = on.then_some(true);
                    }
                    if !on {
                        let surfaces: &[Surface] = if wall { &[Surface::Wall] } else { &[Surface::Shell, Surface::Apps, Surface::Icons, Surface::Web] };
                        let defaults = ColorChoice { auto: Some(true), wall_auto: Some(true), ..Default::default() };
                        for &sf in surfaces {
                            let d = defaults.levels(sf, None, true);
                            match sf {
                                Surface::Shell => {
                                    c.shell_tint = d.tint;
                                    c.shell_sat = None;
                                }
                                Surface::Apps => {
                                    c.apps_tint = d.tint;
                                    c.apps_sat = None;
                                }
                                Surface::Icons => {
                                    c.icons_tint = d.tint.unwrap_or(0);
                                    c.icons_sat = None;
                                }
                                Surface::Web => {
                                    c.web_tint = d.tint;
                                    c.web_sat = None;
                                }
                                Surface::Wall => {
                                    c.wallpaper_tint = d.tint.filter(|t| *t > 0);
                                    c.wall_sat = None;
                                }
                            }
                        }
                        freeze(&mut c, surfaces, frozen);
                    }
                }
                // A surface's Lighting as ONE LINE (0..100, the gap at 46..54).
                "colors-shell-line" | "colors-apps-line" | "colors-icons-line" | "colors-web-line" | "colors-wall-line" => match value.parse::<u8>() {
                    Ok(n) => {
                        let (dark, light) = line_to(n);
                        let (d, l) = match key {
                            "colors-shell-line" => (&mut c.shell_dark, &mut c.shell_light),
                            "colors-apps-line" => (&mut c.apps_dark, &mut c.apps_light),
                            "colors-icons-line" => (&mut c.icons_dark, &mut c.icons_light),
                            "colors-web-line" => (&mut c.web_dark, &mut c.web_light),
                            _ => (&mut c.wall_dark, &mut c.wall_light),
                        };
                        *d = Some(dark);
                        *l = Some(light);
                    }
                    Err(_) => return,
                },
                // A surface's Transparency (the Icons section's is the dock's icons').
                "colors-shell-clear" | "colors-apps-clear" | "colors-web-clear" | "colors-icons-clear" => match value.parse::<u8>() {
                    Ok(n) => {
                        let s = match key {
                            "colors-shell-clear" => Surface::Shell,
                            "colors-apps-clear" => Surface::Apps,
                            "colors-web-clear" => Surface::Web,
                            _ => Surface::Icons,
                        };
                        let v = (n.min(100) != clear_made(s)).then_some(n.min(100));
                        match s {
                            Surface::Shell => c.shell_clear = v,
                            Surface::Apps => c.apps_clear = v,
                            Surface::Web => c.web_clear = v,
                            _ => c.dock_clear = v,
                        }
                    }
                    Err(_) => return,
                },
                // A surface's Saturation.
                "colors-shell-sat" | "colors-apps-sat" | "colors-icons-sat" | "colors-web-sat" | "colors-wall-sat" => match value.parse::<u8>() {
                    Ok(n) => {
                        let v = (n.min(100) != 50).then_some(n.min(100));
                        match key {
                            "colors-shell-sat" => c.shell_sat = v,
                            "colors-apps-sat" => c.apps_sat = v,
                            "colors-icons-sat" => c.icons_sat = v,
                            "colors-web-sat" => c.web_sat = v,
                            _ => c.wall_sat = v,
                        }
                    }
                    Err(_) => return,
                },
                // An app's own look: "<desktop id>:dark|light|follow".
                "colors-app-look" => {
                    let Some((id, look)) = value.rsplit_once(':') else { return };
                    match look {
                        "dark" | "light" => {
                            c.app_look.insert(id.to_owned(), look.to_owned());
                        }
                        "follow" => {
                            c.app_look.remove(id);
                        }
                        _ => return,
                    }
                    set_looked_programs(&self.entries, &c.app_look);
                }
                // The wallpapers' Colours: 0 = the pictures as they are.
                "colors-wallpaper" | "colors-wall-tint" => match value.parse::<u8>() {
                    Ok(n) => c.wallpaper_tint = (n > 0).then_some(n.min(100)),
                    Err(_) => return,
                },
                // "Include the background" of the old box: the wallpaper's Ambient.
                "colors-lighting-veil" => match value {
                    "on" => c.wall_ambient = Some(true),
                    "off" => c.wall_ambient = None,
                    _ => return,
                },
                // An app in its own colours or the palette's: its id turns it.
                "colors-skip" => {
                    if let Some(i) = c.skip.iter().position(|s| s == value) {
                        c.skip.remove(i);
                    } else {
                        c.skip.push(value.to_owned());
                    }
                    set_skipped_programs(&self.entries, &c.skip);
                }
                // A slider's own look: "dark" / "light" (from then on it is the owner's).
                "colors-apps-look" | "colors-web-look" | "colors-icons-look" | "colors-shell-look" => {
                    let dark = match value {
                        "dark" => Some(true),
                        "light" => Some(false),
                        _ => return,
                    };
                    match key {
                        "colors-apps-look" => c.apps_dark = dark,
                        "colors-web-look" => c.web_dark = dark,
                        "colors-shell-look" => c.shell_dark = dark,
                        _ => c.icons_dark = dark,
                    }
                }
                _ => return,
            }
            s.colors = Some(c);
            s.store();
        });
        let c = choice();
        // A wallpaper that follows the colours is made again for the palette
        // or the look just chosen (it does nothing for any other key).
        if matches!(key, "colors-palette" | "colors-shell-look" | "colors-wall-line" | "colors-wall-auto" | "colors-wall-ambient") {
            crate::wall_follow::refresh();
        }
        match key {
            // The shell's look is the dock's whole look (its glass, figures,
            // boxes, the desktop) — what the general Light used to be.
            // (A move within the same look changes no look: the shell wears
            // its new light at once all the same — it waited for the next
            // change of anything, the pass of 2026-10-27.)
            "colors-shell-look" | "colors-shell-line" => {
                let dark = c.levels_now(Surface::Shell).dark;
                if dark == crate::look::dark() {
                    self.colors_apply(false);
                } else {
                    self.look_set(dark);
                }
            }
            // The apps' look is the one the system tells every program.
            "colors-apps-look" | "colors-apps-line" => {
                tell_apps_look(c.levels_now(Surface::Apps).dark);
                self.colors_apply(true);
            }
            // Auto or Ambient turned ON is the owner's own act, and they are
            // plainly there: the light goes to the sun's at once, in front of them.
            "colors-lighting-auto" | "colors-ambient" | "colors-wall-ambient" | "colors-auto" | "colors-wall-auto" if value == "on" => {
                self.look_set(c.levels_now(Surface::Shell).dark);
                self.colors_apply(true);
                self.lighting_auto_on();
            }
            "colors-auto" | "colors-ambient" | "colors-lighting-auto" => {
                self.look_set(c.levels_now(Surface::Shell).dark);
                self.colors_apply(true);
            }
            // An app's own colours or look changes no colour of the theme, so
            // nothing was written: Seam's file says which webapps keep their own
            // colours and which have a look of their own, and is written now.
            "colors-skip" | "colors-app-look" => {
                self.colors_apply(false);
                publish(current(), false);
            }
            _ => self.colors_apply(false),
        }
    }
}

/// The legibility audit: every pair of words over a ground, as seen.
#[cfg(test)]
#[path = "palette_audit.rs"]
mod audit;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_rules_and_the_word_on_closing() {
        // Each rule on only while it shows anything; the web's rides the
        // apps' when it has none of its own (it is declared later and wins).
        let at = |apps, web| clear_rules_lua(&ColorChoice { auto: Some(false), apps_clear: apps, web_clear: web, ..Default::default() });
        let on = |l: &str| (l.contains("1.0\", enabled = true }) hl"), l.ends_with("enabled = true }) end)"));
        assert_eq!(on(&at(None, None)), (false, false));
        assert_eq!(on(&at(Some(60), None)), (true, true));
        assert_eq!(on(&at(None, Some(60))), (false, true));
        assert!(at(Some(60), None).contains("opacity = \"0.43 0.43 1.0\""));
        assert!(at(None, None).starts_with("pcall(function() "));
        // The word as Colours closes: only for MORE transparency, past as made.
        assert!(rose([35, 0, 0, 0], [60, 0, 0, 0]));
        assert!(rose([35, 0, 0, 0], [35, 0, 0, 10]));
        assert!(!rose([60, 0, 0, 0], [35, 0, 0, 0]));
        assert!(!rose([0, 0, 0, 0], [20, 0, 0, 0]));
        assert!(!rose([35, 40, 0, 0], [35, 40, 0, 0]));
    }

    #[test]
    fn a_swatch_is_the_ground_of_the_look_then_the_soul() {
        let p = all().into_iter().find(|p| p.id == "jade").expect("a curated palette");
        let dark = swatch(&p, true);
        let light = swatch(&p, false);
        assert_eq!((dark[0], light[0]), (p.dark[0], p.light[0]));
        assert_eq!(dark[1..], p.soul[..]);
        assert_eq!(dark[1..], light[1..], "the soul is the same in both looks");
        // A palette with no soul of its own shows the bright half of its sixteen.
        let bare = Palette { soul: Vec::new(), ..p.clone() };
        assert_eq!(swatch(&bare, true)[1..], p.dark[8..16]);
    }

    #[test]
    fn a_drawing_for_the_apps_takes_the_icon_filter() {
        let mut f = [[0.0f32; 4]; 7];
        f[0] = [1.0, 0.0, 1.0, 0.0];
        f[1] = [0.6, 0.15, 330f32.to_radians(), 0.0];
        let svg = r##"<path fill="#808080" id="a#b"/><stop style="stop-color:#fff"/>"##;
        let out = tint_svg(svg, &f);
        assert!(out.contains(r#"id="a#b""#), "not a colour: left alone");
        let grey = &out[out.find("fill=\"#").unwrap() + 7..][..6];
        assert_ne!(grey, "808080", "a grey takes the palette's colour");
        let (_, c, _) = tint::lch([u8::from_str_radix(&grey[0..2], 16).unwrap(), u8::from_str_radix(&grey[2..4], 16).unwrap(), u8::from_str_radix(&grey[4..6], 16).unwrap()]);
        assert!(c > 0.05, "clearly coloured, not a hint ({c})");
        // Not colours: an entity, a link to a gradient, an href.
        let links = r##"<t>&#160;</t><g fill="url(#abcdef)"/><use href="#abc"/>"##;
        assert_eq!(tint_svg(links, &f), links);
        // Off: nothing changes.
        f[0][0] = 0.0;
        assert_eq!(tint_svg(svg, &f), svg.replace("#fff", "#ffffff"));
    }

    fn golem_choice() -> ColorChoice {
        ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }
    }

    /// A theme of a palette as made, in a look.
    fn plain(p: &Palette, dark: bool) -> Theme {
        Theme {
            id: p.id.clone(),
            name: p.name.clone(),
            dark,
            dynamic: false,
            apps: true,
            web: false,
            c: if dark { p.dark } else { p.light },
            accent_slot: if dark { p.accent } else { p.accent_light },
            apps_tint: None,
            web_tint: Some(0),
            icons_tint: 0,
            apps_dark: dark,
            web_dark: dark,
            icons_dark: dark,
            apps_light: 50,
            apps_sat: 50,
            web_light: 50,
            web_sat: 50,
            icons_sat: 50,
            pair: None,
            looks: None,
        }
    }

    #[test]
    fn the_light_shell_is_the_designed_glass_never_a_palettes_paper() {
        // (Max, 2026-10-21: "the shell should not go white".)
        for palette in [GOLEM, "chartreuse"] {
            for tint in [None, Some(0), Some(100)] {
                let picked = ColorChoice { palette: Some(palette.to_owned()), shell_tint: tint, ..Default::default() };
                assert!(matches!(shell_plan_look(&picked, false), ShellPlan::Screen), "{palette} at {tint:?}: the light shell reads the screen");
                assert!(!matches!(shell_plan_look(&picked, true), ShellPlan::Screen), "{palette} at {tint:?}: the dark shell wears its ground");
            }
        }
    }

    #[test]
    fn the_shells_pair_at_any_lighting_is_the_walked_palette_lit() {
        let picked = ColorChoice { palette: Some(GOLEM.to_owned()), shell_dark: Some(true), ..Default::default() };
        let plan = shell_plan(&picked, true);
        assert!(plan.follows_lighting());
        let p = golem();
        for v in [0u8, 20, 50, 80, 100] {
            let (surface, ink) = plan.pair(v).expect("a palette gives a pair");
            // The old way: walk, light, make readable, read the surface and the ink.
            let mut c = tint::lit(&tint::walk_unlit(&p.dark, p.dark[p.accent], true, None), true, v);
            readable(&mut c, true, p.accent);
            assert_eq!((surface, ink), (linear(c[0x01]), linear(c[0x05])), "at {v}");
        }
        let (dim, _) = plan.pair(10).unwrap();
        let (bright, _) = plan.pair(90).unwrap();
        assert!(dim[0] <= bright[0] && dim != bright, "less light, a darker ground");
        // Off is Golem's own neutral pair: the Lighting does not move it.
        let off = ColorChoice { shell_tint: Some(0), ..picked.clone() };
        let plan = shell_plan(&off, true);
        assert!(!plan.follows_lighting());
        assert_eq!(plan.pair(10), plan.pair(90));
        // Dynamic: the shell reads the screen, there is no pair.
        let dynamic = ColorChoice { palette: Some(DYNAMIC_ID.to_owned()), ..Default::default() };
        assert!(shell_plan(&dynamic, true).pair(30).is_none());
    }

    #[test]
    fn auto_lighting_is_saved_only_when_on() {
        let c = ColorChoice { lighting_auto: Some(true), lighting_veil: Some(true), ..Default::default() };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("lighting_auto") && json.contains("lighting_veil"));
        let back: ColorChoice = serde_json::from_str(&json).unwrap();
        assert_eq!((back.lighting_auto, back.lighting_veil), (Some(true), Some(true)));
        let json = serde_json::to_string(&ColorChoice::default()).unwrap();
        assert!(!json.contains("lighting"), "off writes nothing: {json}");
        let old: ColorChoice = serde_json::from_str(r#"{"lighting": 30}"#).unwrap();
        assert_eq!((old.lighting, old.lighting_auto, old.lighting_veil), (Some(30), None, None));
    }

    #[test]
    fn the_writing_area_gets_a_scheme_of_the_palette() {
        let t = for_apps(&resolve(&ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }, true));
        let x = sourceview_scheme(&t, "golem-a");
        assert!(x.contains(r#"<style-scheme id="golem-a""#));
        assert!(x.contains(&format!(r##"<style name="text" foreground="#{}" background="#{}"/>"##, hex(t.c[5]), hex(t.c[0]))));
        assert_eq!(x.matches("<style ").count(), 23);
    }

    /// THE SWEEP (Max, 2026-10-16: "i wonder how many other bugs like that
    /// we have"): every palette × every look × slider levels × the system's
    /// look, against rules that must always hold. Reports every breach.
    /// `cargo test -- --ignored the_sweep --nocapture`
    #[test]
    #[ignore]
    fn the_sweep() {
        use std::collections::BTreeMap;
        let mut bad: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut note = |rule: &'static str, what: String| bad.entry(rule).or_default().push(what);
        let chroma = |c: Rgb| tint::lch(c).1;
        let lum = rel_lum;
        let ratio = |a: Rgb, b: Rgb| contrast(rel_lum(a), rel_lum(b));
        let mut ids: Vec<String> = all().into_iter().map(|p| p.id).collect();
        ids.push(GOLEM.to_owned());
        let levels = [Some(0u8), Some(1), Some(25), None, Some(75), Some(100)];
        let looks = [None, Some(true), Some(false)];
        let mut n = 0;
        for id in &ids {
            for sys in [true, false] {
                for lv in levels {
                    for look in looks {
                        n += 1;
                        let tag = format!("{id} sys={} lvl={lv:?} look={look:?}", if sys { "dark" } else { "light" });
                        // ---- APPS ----
                        let c = ColorChoice { palette: Some(id.clone()), apps_tint: lv, apps_dark: look, ..Default::default() };
                        let t = for_apps(&resolve(&c, sys));
                        let dark = look.unwrap_or(sys);
                        if t.dark != dark {
                            note("A1 apps look not the asked one", tag.clone());
                        }
                        if dark && lum(t.c[0]) > 0.2 || !dark && lum(t.c[0]) < 0.35 {
                            note("A2 apps ground the wrong brightness for its look", format!("{tag} ground #{}", hex(t.c[0])));
                        }
                        for g in [0, 1] {
                            if ratio(t.c[5], t.c[g]) < 4.5 {
                                note("A3 apps text under 4.5:1 on a ground", format!("{tag} base05 #{} on base0{g} #{} = {:.2}", hex(t.c[5]), hex(t.c[g]), ratio(t.c[5], t.c[g])));
                            }
                        }
                        if ratio(t.accent(), t.c[1]) < 3.0 {
                            note("A4 apps accent under 3:1 on the surface", format!("{tag} accent #{} on #{} = {:.2}", hex(t.accent()), hex(t.c[1]), ratio(t.accent(), t.c[1])));
                        }
                        for k in 8..16 {
                            if ratio(t.c[k], t.c[0]) < 2.5 {
                                note("A5 a terminal hue under 2.5:1 on the ground", format!("{tag} base{k:02X} #{} on #{} = {:.2}", hex(t.c[k]), hex(t.c[0]), ratio(t.c[k], t.c[0])));
                            }
                        }
                        if lv == Some(0) && (0..8).any(|k| chroma(t.c[k]) > 0.02) {
                            note("A6 apps Off still coloured", format!("{tag} grounds {:?}", (0..8).map(|k| hex(t.c[k])).collect::<Vec<_>>()));
                        }
                        let ini = foot_ini(&t);
                        let want = if dark { "initial-color-theme=dark" } else { "initial-color-theme=light" };
                        if !ini.contains(want) {
                            note("A7 foot starts in the other look", tag.clone());
                        }
                        // GTK 4 picks its half by the SYSTEM's look: an apps look of
                        // its own must be in both halves.
                        if look.is_some_and(|d| d != sys) {
                            let css = gtk4_css(&t);
                            let want = format!("--window-bg-color: #{};", hex(t.c[1]));
                            if css.matches(&want).count() < 2 {
                                note("A8 GTK 4 not in the apps' own look under the system's", tag.clone());
                            }
                        }
                        // ---- THE SHELL (the dock's boxes when not Dynamic) ----
                        let sh = resolve(&ColorChoice { palette: Some(id.clone()), ..Default::default() }, sys);
                        if ratio(sh.ink(), sh.surface()) < 4.5 {
                            note("S1 shell text under 4.5:1 on its surface", format!("{id} sys={sys} ink #{} on #{}", hex(sh.ink()), hex(sh.surface())));
                        }
                        if ratio(sh.accent(), sh.surface()) < 3.0 {
                            note("S2 shell accent under 3:1 on its surface", format!("{id} sys={sys} accent #{} on #{} = {:.2}", hex(sh.accent()), hex(sh.surface()), ratio(sh.accent(), sh.surface())));
                        }
                        if ratio(sh.accent_ink(), sh.accent()) < 4.5 {
                            note("S3 text on the accent under 4.5:1", format!("{id} sys={sys} #{} on #{}", hex(sh.accent_ink()), hex(sh.accent())));
                        }
                        // ---- WEB ----
                        let c = ColorChoice { palette: Some(id.clone()), web_tint: lv, web_dark: look, ..Default::default() };
                        let shell = resolve(&c, sys);
                        let json: serde_json::Value = serde_json::from_str(&colors_json(&for_apps(&shell), &shell, &c, &[])).unwrap();
                        let on = json["web"].as_bool().unwrap_or(false);
                        let bg = parse_hex(json["web_bg"].as_str().unwrap_or("")).unwrap_or([0; 3]);
                        let ink = parse_hex(json["web_ink"].as_str().unwrap_or("")).unwrap_or([0; 3]);
                        let wdark = json["web_dark"].as_bool().unwrap_or(true);
                        let expect_on = lv != Some(0) || look == Some(true);
                        if on != expect_on {
                            note("W1 web on/off not what the slider + look say", format!("{tag} web={on}"));
                        }
                        if on {
                            if wdark != dark {
                                note("W2 web look not the asked one", format!("{tag} web_dark={wdark}"));
                            }
                            if wdark && lum(bg) > 0.2 || !wdark && lum(bg) < 0.35 {
                                note("W3 web ground the wrong brightness for its look", format!("{tag} bg #{}", hex(bg)));
                            }
                            if ratio(ink, bg) < 4.5 {
                                note("W4 web text under 4.5:1", format!("{tag} ink #{} on #{} = {:.2}", hex(ink), hex(bg), ratio(ink, bg)));
                            }
                            if lv == Some(0) && (chroma(bg) > 0.01 || chroma(ink) > 0.01) {
                                note("W5 web Off still coloured", format!("{tag} bg #{} ink #{}", hex(bg), hex(ink)));
                            }
                        }
                    }
                }
            }
        }
        println!("\nTHE SWEEP: {n} cases, {} palettes", ids.len());
        for (rule, list) in &bad {
            let mut pals: Vec<&str> = list.iter().map(|w| w.split(' ').next().unwrap_or("")).collect();
            pals.dedup();
            println!("\n{rule}: {} cases, palettes {:?}", list.len(), { let mut p = pals.clone(); p.sort(); p.dedup(); p });
            let mut by: BTreeMap<String, usize> = BTreeMap::new();
            for w in list { let k: Vec<&str> = w.split(' ').skip(1).take(3).collect(); *by.entry(k.join(" ")).or_default() += 1; }
            println!("    by case: {by:?}");
            for w in list.iter().take(4) {
                println!("    {w}");
            }
        }
        assert!(bad.is_empty(), "{} rules broken", bad.len());
    }

    #[test]
    fn off_takes_the_colour_not_the_dark() {
        let off = |web_dark| {
            let c = ColorChoice { palette: Some(GOLEM.to_owned()), web_tint: Some(0), web_dark, ..Default::default() };
            let t = resolve(&c, true);
            let json: serde_json::Value = serde_json::from_str(&colors_json(&for_apps(&t), &t, &c, &[])).unwrap();
            json
        };
        // Its Dark clicked: the web stays dark, in greys with no colour.
        let dark = off(Some(true));
        assert_eq!(dark["web"], true);
        assert_eq!(dark["web_dark"], true);
        let bg = parse_hex(dark["web_bg"].as_str().unwrap()).unwrap();
        assert!(tint::lch(bg).1 < 0.01, "neutral, not the palette's colour");
        // A palette whose ink is coloured (Hacker): the ink goes grey too.
        let c = ColorChoice { palette: Some("hacker".to_owned()), web_tint: Some(0), web_dark: Some(true), ..Default::default() };
        let t = resolve(&c, true);
        let json: serde_json::Value = serde_json::from_str(&colors_json(&for_apps(&t), &t, &c, &[])).unwrap();
        let ink = parse_hex(json["web_ink"].as_str().unwrap()).unwrap();
        assert!(tint::lch(ink).1 < 0.01, "Hacker's green ink, greyed: {ink:?}");
        // Left to follow the system, Off leaves the sites as they are made.
        assert_eq!(off(None)["web"], false);
    }

    #[test]
    fn a_program_is_known_by_its_first_word() {
        assert_eq!(program_of("env FOO=1 /usr/bin/gnome-text-editor --new-window"), "gnome-text-editor");
        assert_eq!(program_of("vlc --started-from-file"), "vlc");
        assert_eq!(program_of(""), "");
    }

    #[test]
    fn the_old_choices_carry_over() {
        let read = |j: &str| serde_json::from_str::<ColorChoice>(j).unwrap();
        // Nothing chosen: Dynamic, the apps as made, the web off, the icons off.
        let none = read("{}");
        assert_eq!((none.palette_id(), none.apps_level(), none.web_level(), none.icons_tint), (DYNAMIC_ID, None, Some(0), 0));
        // The old switch on (the screen's colours): Dynamic, whatever palette it named.
        assert_eq!(read(r#"{"dynamic":true,"palette":"nord"}"#).palette_id(), DYNAMIC_ID);
        // The old switches: apps off → 0; dark websites on → the web as made.
        let old = read(r#"{"dynamic":false,"palette":"fluor","dark_websites":true,"apps":false}"#);
        assert_eq!((old.palette_id(), old.apps_level(), old.web_level()), ("fluor", Some(0), None));
        // Written back: the sliders, no old switch; as made survives the trip.
        let back = serde_json::to_value(old.settled()).unwrap();
        assert_eq!(back, serde_json::json!({"palette": "fluor", "apps_tint": 0, "web_tint": null, "icons_tint": 0}));
        assert_eq!(read(&back.to_string()).web_level(), None);
    }

    #[test]
    fn a_web_change_leaves_the_programs_alone() {
        let t = resolve(&golem_choice(), true);
        assert!(same_for_apps(&for_apps(&t), &for_apps(&Theme { web: true, web_tint: Some(80), ..t.clone() })));
        assert!(!same_for_apps(&for_apps(&t), &for_apps(&Theme { apps_tint: Some(90), ..t.clone() })));
        assert!(!same_for_apps(&for_apps(&t), &for_apps(&Theme { apps_dark: false, ..t.clone() })));
    }

    #[test]
    fn apps_off_wear_the_makers_colours_with_the_palettes_accent() {
        let t = resolve(&golem_choice(), true);
        let s = stock(&t);
        let off = for_apps(&Theme { apps_tint: Some(0), apps: false, ..t.clone() });
        assert_eq!(off.c[1], s.c[1], "the makers' ground");
        assert_eq!(off.accent(), t.accent(), "the palette's accent kept");
        for dark in [true, false] {
            let s = stock(&Theme { dark, ..t.clone() });
            assert_eq!(rel_lum(s.bg()) < 0.18, dark);
            assert!(contrast(rel_lum(s.surface()), rel_lum(s.ink())) >= 4.5);
        }
        // The apps' own look: light apps on a dark system.
        let light_apps = for_apps(&Theme { apps_dark: false, ..t.clone() });
        assert!(!light_apps.dark && rel_lum(light_apps.bg()) > 0.5);
    }

    /// THE PAIRS: every secondary pair a program's theme makes (links, the
    /// accent as text, selections, dim text, buttons), at every Lighting —
    /// the sweep's cousin for the halves that come from two places. Prints.
    #[test]
    #[ignore]
    fn the_pairs() {
        use std::collections::BTreeMap;
        let ratio = |a: Rgb, b: Rgb| contrast(rel_lum(a), rel_lum(b));
        let mut bad: BTreeMap<String, (usize, f32, String, std::collections::BTreeSet<String>)> = BTreeMap::new();
        let mut n = 0;
        let ids: Vec<String> = all().into_iter().map(|p| p.id).collect();
        for lg in [0u8, 50, 100] {
            tint::set_lighting(lg);
            for id in &ids {
                for sys in [true, false] {
                    for lv in [Some(0u8), None, Some(100)] {
                        for look in [None, Some(true), Some(false)] {
                            let c = ColorChoice { palette: Some(id.clone()), apps_tint: lv, apps_dark: look, shell_tint: lv, shell_dark: look, ..Default::default() };
                            let sh = resolve(&c, sys);
                            let t = for_apps(&sh);
                            let k = &t.c;
                            let tag = format!("{id} sys={} lvl={lv:?} look={look:?} light={lg}", if sys { "dark" } else { "light" });
                            let mut check = |rule: &str, fg: Rgb, bg: Rgb, floor: f32| {
                                n += 1;
                                let r = ratio(fg, bg);
                                if r < floor {
                                    let e = bad.entry(format!("{rule} @{lg}{}", if lv == Some(100) { " full" } else if lv == Some(0) { " off" } else { "" })).or_insert((0, 99.0, String::new(), Default::default()));
                                    e.0 += 1;
                                    e.3.insert(id.clone());
                                    if r < e.1 { e.1 = r; e.2 = format!("{tag} #{} on #{}", hex(fg), hex(bg)); }
                                }
                            };
                            for (g, gn) in [(0, "base00"), (1, "base01")] {
                                check(&format!("P1 accent as text on {gn} <4.5"), t.accent_text(), k[g], 4.5);
                                check(&format!("P2 link 0D on {gn} <4.5"), t.as_text(k[0xD]), k[g], 4.5);
                                check(&format!("P2 link 0E on {gn} <4.5"), t.as_text(k[0xE]), k[g], 4.5);
                                check(&format!("P3 ink_dim on {gn} <3"), t.ink_dim(), k[g], 3.0);
                                for s in [0x8, 0xA, 0xB] {
                                    check(&format!("P8 semantic {s:02X} as text on {gn} <4.5"), t.as_text(k[s]), k[g], 4.5);
                                }
                            }
                            check("P3 ink_dim on base02 <3", t.ink_dim(), k[2], 3.0);
                            check("P4 ink on base02 (buttons, inputs) <4.5", t.ink(), k[2], 4.5);
                            check("P4 ink on base00 <4.5", t.ink(), k[0], 4.5);
                            check("P5 ink on the editor selection <4.5", t.ink(), t.selection(), 4.5);
                            check("P6 base06 on base02 (sourceview selection) <4.5", k[6], k[2], 4.5);
                            check("P6 base05 on base02 (foot selection, tooltips) <4.5", k[5], k[2], 4.5);
                            check("P6 base07 on base02 (btop selection) <4.5", k[7], k[2], 4.5);
                            check("P9 accent as text on base02 <3", t.accent_text(), k[2], 3.0);
                            check("P10 apps text on the accent <4.5", t.accent_ink(), t.accent(), 4.5);
                            check("S1 shell ink on its surface <4.5", sh.ink(), sh.surface(), 4.5);
                            check("S2 shell accent on its surface <3", sh.accent(), sh.surface(), 3.0);
                            check("S3 shell text on its accent <4.5", sh.accent_ink(), sh.accent(), 4.5);
                        }
                    }
                }
            }
        }
        tint::set_lighting(50);
        println!("\nTHE PAIRS: {n} checks, {} palettes", ids.len());
        for (rule, (count, worst, ex, pals)) in &bad {
            let some: Vec<&String> = pals.iter().take(8).collect();
            let _ = some; println!("{rule}: {count} / {} pal, worst {worst:.2} ({ex})", pals.len());
        }
    }

    #[test]
    fn text_reads_at_every_lighting() {
        // (THE PAIRS found the Lighting walking the grounds out from under the
        // inks, and the accent and links too faint as text — 2026-10-20.)
        let r = |a: Rgb, b: Rgb| contrast(rel_lum(a), rel_lum(b));
        for p in all().iter().take(116) {
            for dark in [true, false] {
                let base = plain(p, dark);
                for level in [None, Some(100)] {
                    let made = tint::walk_unlit(&base.c, base.accent(), dark, level);
                    for v in [0u8, 20, 50, 100] {
                        let t = Theme { c: tint::lit(&made, dark, v), ..base.clone() };
                        let tag = format!("{} dark={dark} level={level:?} lighting={v}", p.id);
                        for g in 0..3 {
                            assert!(r(t.ink(), t.c[g]) >= 4.5, "{tag}: ink on base0{g}");
                            assert!(r(t.ink_dim(), t.c[g]) >= 3.0, "{tag}: ink_dim on base0{g}");
                            let floor = if g == 2 { 3.0 } else { 4.5 };
                            for x in [t.accent(), t.c[0xD], t.c[0xE], t.c[0x8], t.c[0xA], t.c[0xB]] {
                                assert!(r(t.as_text(x), t.c[g]) >= floor, "{tag}: #{} as text on base0{g}", hex(x));
                            }
                        }
                        assert!(r(t.accent_ink(), t.accent()) >= 4.5, "{tag}: text on the accent");
                        assert!(r(t.ink(), t.selection()) >= 4.5, "{tag}: text on an editor's selection");
                    }
                }
            }
        }
    }

    #[test]
    fn under_ambient_the_pages_pair_holds_through_the_light() {
        // (Every new pair restarts Dark Reader: every open page flashes.)
        let pair = |ambient: bool, light: u8| {
            let picked = ColorChoice { ambient: Some(ambient), web_tint: None, ..Default::default() };
            let shell = Theme { web_light: light, apps_light: light, web_dark: true, ..resolve(&picked, true) };
            let j: serde_json::Value = serde_json::from_str(&colors_json(&for_apps(&shell), &shell, &picked, &[])).unwrap();
            (j["web_bg"].as_str().unwrap().to_owned(), j["apps_bg"].as_str().unwrap().to_owned())
        };
        assert_eq!(pair(true, 30), pair(true, 60), "Ambient: one pair a look");
        assert_ne!(pair(false, 30), pair(false, 60), "by hand: the Lighting reaches the pages");
    }

    #[test]
    fn auto_is_auto_no_app_keeps_an_exception_under_it() {
        let mut c = ColorChoice { skip: vec!["org.inkscape.Inkscape".into()], ..Default::default() };
        c.app_look.insert("code".into(), "dark".into());
        assert_eq!(c.skip_in_force().len(), 1, "manual: the exceptions hold");
        assert_eq!(c.looks_in_force().len(), 1);
        c.auto = Some(true);
        assert!(c.skip_in_force().is_empty(), "Auto: every app follows the section");
        assert!(c.looks_in_force().is_empty());
        assert_eq!(c.skip.len(), 1, "kept, for Auto turned off");
        c.auto = Some(false);
        assert_eq!(c.skip_in_force().len(), 1, "back as they were");
    }

    #[test]
    fn an_app_starts_in_its_own_colours_or_its_own_look() {
        // (One test owns the two tables: tests run side by side.)
        *SKIPPED.lock().unwrap() = vec!["vlc".into()];
        *LOOKED.lock().unwrap() = vec![("vlc".into(), false), ("gimp".into(), true), ("blender".into(), false)];
        assert_eq!(skip_vars("/usr/bin/foot -e x"), Vec::new(), "an app that follows: nothing");
        let vlc = skip_vars("vlc %U");
        assert!(vlc.contains(&("GTK_THEME", "Adwaita".to_owned())) && vlc.iter().any(|(k, _)| *k == "QT_QPA_PLATFORMTHEME"), "own colours, in its own Light: {vlc:?}");
        assert_eq!(skip_vars("env FOO=1 gimp"), vec![("GTK_THEME", "Golem-Dark:dark".to_owned())], "the palette, pinned Dark");
        assert_eq!(skip_vars("blender"), vec![("GTK_THEME", "Golem-Light".to_owned())], "the palette, pinned Light");
        SKIPPED.lock().unwrap().clear();
        LOOKED.lock().unwrap().clear();
    }

    #[test]
    fn a_program_pinned_to_a_look_wears_that_look_whatever_the_apps_keep() {
        // (The pass of 2026-10-27: Apps Light under a Dark shell gave the
        // apps' pair Light in both halves, and a Dark app wore it.)
        for apps_tint in [None, Some(0)] {
            let c = ColorChoice { palette: Some(GOLEM.into()), shell_dark: Some(true), apps_dark: Some(false), apps_tint, ..ColorChoice::default() };
            let t = for_apps(&resolve(&c, true));
            let (pd, _, pl, _) = t.pair.expect("the apps' pair");
            assert_eq!(pd, pl, "the apps' own files keep the apps' look ({apps_tint:?})");
            let (dc, _, lc, _) = t.looks.expect("both looks");
            assert!(tint::lch(dc[1]).0 < 0.5 && tint::lch(lc[1]).0 > 0.5, "Dark is dark and Light is light ({apps_tint:?})");
        }
    }

    #[test]
    fn the_line_round_trips_through_both_looks() {
        // (The box's one Lighting slider: 0..45 Dark, 55..100 Light, the gap between.)
        for light in [0u8, 25, 50, 75, 100] {
            for dark in [true, false] {
                let v = line_of(dark, light);
                let (d, l) = line_to(v);
                assert_eq!(d, dark, "{dark} {light} -> {v}");
                assert!((i16::from(l) - i16::from(light)).abs() <= 1, "{dark} {light} -> {v} -> {l}");
                if dark { assert!(v <= 45) } else { assert!(v >= 55) }
            }
        }
        assert_eq!(line_to(47), (true, 100), "the gap's dark side is Dark 100");
        assert_eq!(line_to(53), (false, 0), "the gap's light side is Light 0");
        assert_eq!(line_of(true, 50), 23);
        assert_eq!(line_of(false, 50), 78);
    }

    #[test]
    fn the_levels_are_the_owners_the_suns_or_golems() {
        let own = ColorChoice { apps_tint: Some(80), apps_sat: Some(20), apps_dark: Some(false), apps_light: Some(70), ..Default::default() };
        let l = own.levels(Surface::Apps, Some((true, 10)), true);
        assert_eq!(l, Levels { dark: false, light: 70, tint: Some(80), sat: 20 }, "manual: the owner's, the sun ignored");
        let amb = ColorChoice { ambient: Some(true), ..own.clone() };
        let l = amb.levels(Surface::Apps, Some((true, 10)), true);
        assert_eq!(l, Levels { dark: true, light: 10, tint: Some(80), sat: 20 }, "Ambient: the sun's look and light, the owner's colours");
        let l = amb.levels(Surface::Apps, None, true);
        assert_eq!(l.light, 70, "no sun yet: the owner's");
        let auto = ColorChoice { auto: Some(true), ..own.clone() };
        let l = auto.levels(Surface::Apps, Some((false, 60)), true);
        assert_eq!(l, Levels { dark: false, light: 60, tint: None, sat: 50 }, "Auto: Golem's defaults, the sun's light");
        assert_eq!(auto.levels(Surface::Icons, None, true).tint, Some(0), "Auto: icons untinted");
        assert_eq!(auto.levels(Surface::Wall, None, true).tint, Some(0), "the wallpaper's Auto is its own tab's");
        let wall = ColorChoice { wall_auto: Some(true), ..Default::default() };
        assert_eq!(wall.levels(Surface::Wall, None, true), Levels { dark: true, light: 50, tint: Some(0), sat: 50 }, "Auto: Palette 0%, Saturation as made");
        assert!(wall.ambient_of(Surface::Wall) && !wall.ambient_of(Surface::Shell), "each tab's Auto brings its own Ambient");
        // A fresh ColorChoice is manual (a fresh INSTALL is set to Auto by settle_lines).
        assert!(!ColorChoice::default().auto_of(Surface::Apps));
    }

    #[test]
    fn saturation_keeps_what_reads() {
        // (THE SATURATION axis, 2026-10-20: grey or vivid, the inks still read
        // on the grounds and the accent still shows on the surface.)
        let r = |a: Rgb, b: Rgb| contrast(rel_lum(a), rel_lum(b));
        for p in all().iter().take(116) {
            for dark in [true, false] {
                let (base, acc) = if dark { (p.dark, p.accent) } else { (p.light, p.accent_light) };
                for sat in [0u8, 25, 75, 100] {
                    for level in [None, Some(100)] {
                        let mut c = tint::walk(&base, p.dark[p.accent], dark, level, 50, sat);
                        readable(&mut c, dark, acc);
                        let tag = format!("{} dark={dark} sat={sat} level={level:?}", p.id);
                        for g in 0..3 {
                            assert!(r(c[5], c[g]) >= 4.5, "{tag}: ink on base0{g} {:.2}", r(c[5], c[g]));
                            assert!(r(c[4], c[g]) >= 3.0, "{tag}: ink_dim on base0{g}");
                        }
                        assert!(r(c[acc], c[1]) >= 3.0, "{tag}: the accent on the surface");
                        // (Saturation alone moves no lightness: the walk may, the chroma change may not.)
                        let unsat = tint::walk(&base, p.dark[p.accent], dark, level, 50, 50);
                        let (l0, _, _) = tint::lch(unsat[1]);
                        let (l1, _, _) = tint::lch(tint::saturate(&unsat, sat)[1]);
                        assert!((l0 - l1).abs() < 0.02, "{tag}: the surface keeps its lightness ({l0:.3} -> {l1:.3})");
                    }
                }
                let grey = tint::saturate(&base, 0);
                assert!(grey.iter().all(|x| tint::lch(*x).1 < 0.01), "{} dark={dark}: 0 is grey", p.id);
            }
        }
    }

    #[test]
    fn what_sits_on_a_selection_reads_on_it() {
        // (Seam's chosen suggestion: white on India Ink's light accent — Max, 2026-10-20.)
        let rgb = |s: &str| -> Rgb {
            let v: Vec<u8> = s.split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        };
        for p in all().iter().take(116) {
            for dark in [true, false] {
                let t = plain(p, dark);
                let r = contrast(rel_lum(t.accent_ink()), rel_lum(t.accent()));
                assert!(r >= 4.5, "{} dark={dark}: text on the accent {r:.2}", p.id);
                let kde = crate::app_themes::kde_scheme(&t);
                let sel = &kde.iter().find(|(s, _)| s == "Colors:Selection").unwrap().1;
                let bg = rgb(&sel.iter().find(|(k, _)| k == "BackgroundNormal").unwrap().1);
                for (k, v) in sel.iter().filter(|(k, _)| k.starts_with("Foreground")) {
                    let floor = if k == "ForegroundInactive" { 3.0 } else { 4.5 };
                    let r = contrast(rel_lum(rgb(v)), rel_lum(bg));
                    assert!(r >= floor, "{} dark={dark}: KDE Selection {k} {r:.2}", p.id);
                }
            }
        }
    }

    #[test]
    fn the_curated_box_is_whole_and_reads() {
        let all = all();
        assert!(all.len() >= 116, "{}", all.len());
        assert_eq!(all[0].id, "fog", "the box starts with the greys");
        for p in all.iter().take(116) {
            assert_eq!(p.soul.len(), 6, "{}: six capsules", p.id);
            for dark in [true, false] {
                let t = plain(p, dark);
                assert!(contrast(rel_lum(t.surface()), rel_lum(t.ink())) >= 7.0, "{} dark={dark}: ink", p.id);
                assert!(contrast(rel_lum(t.surface()), rel_lum(t.accent())) >= 3.0, "{} dark={dark}: accent", p.id);
            }
        }
        let klimt = all.iter().find(|p| p.id == "klimt").unwrap();
        assert_ne!(klimt.accent, klimt.accent_light, "gold does not read on paper: another accent there");
    }

    #[test]
    fn a_picture_gives_a_palette_of_its_own() {
        // A forest green picture with some grey: a green accent, greenish greys.
        let mut px = vec![[0x2e, 0x7d, 0x32]; 600];
        px.extend(vec![[0x80, 0x80, 0x80]; 400]);
        let p = from_pixels(&px);
        assert_eq!(p.accent, 0x0B, "green, in green's slot");
        for (dark, c) in [(true, p.dark), (false, p.light)] {
            let t = Theme { c, dynamic: true, ..plain(&p, dark) };
            assert_eq!(rel_lum(t.bg()) < 0.18, dark, "dark={dark}");
            assert!(contrast(rel_lum(t.surface()), rel_lum(t.ink())) >= 4.5, "dark={dark}: ink reads");
            assert!(contrast(rel_lum(t.surface()), rel_lum(t.accent())) >= 3.0, "dark={dark}: the accent shows");
            let (_, _, _) = hsl(t.bg());
            assert!(t.bg()[1] >= t.bg()[0] && t.bg()[1] >= t.bg()[2], "dark={dark}: greys lean green {:?}", t.bg());
        }
        // A grey picture: plain greys, the blue accent.
        let grey = from_pixels(&vec![[0x77, 0x77, 0x77]; 100]);
        assert_eq!(grey.accent, 0x0D);
        assert_eq!(grey.dark[1][0], grey.dark[1][2]);
        // A red sunset: the accent in red's slot or orange's.
        let red = from_pixels(&vec![[0xd8, 0x40, 0x30]; 100]);
        assert!(matches!(red.accent, 0x08 | 0x09), "{:x}", red.accent);
    }

    #[test]
    fn golems_palette_reads_and_resolves() {
        let p = parse(GOLEM, BUILT_IN[0].1).expect("parses");
        assert_eq!(p.name, "Golem");
        assert_eq!(p.accent, 0x09);
        assert_eq!(p.dark[0x09], [0xff, 0xbe, 0x98], "the desktop's peach");
        let dark = resolve(&golem_choice(), true);
        assert!(!dark.dynamic && dark.dark);
        assert_eq!(dark.accent(), [0xff, 0xbe, 0x98]);
        assert!(rel_lum(dark.bg()) < rel_lum(dark.ink()), "dark: light ink on a dark ground");
        let light = resolve(&golem_choice(), false);
        assert!(!light.dynamic);
        assert_eq!(light.accent_slot, 0x09, "the palette's own accent, always");
        // An old settings file that still names an accent of its own: ignored.
        let old: ColorChoice = serde_json::from_str(r#"{"dynamic":false,"accent":"base0D"}"#).unwrap();
        assert_eq!(old.palette_id(), GOLEM);
        assert_eq!(resolve(&old, true).accent_slot, 0x09);
        assert!(rel_lum(light.bg()) > rel_lum(light.ink()), "light: dark ink on a light ground");
    }

    #[test]
    fn golems_light_is_the_files_window_max_tuned() {
        // 2026-10-14: Files' light look, tuned by eye — the palette carries it now.
        let t = resolve(&golem_choice(), false);
        assert_eq!(t.surface(), [0xdc, 0xd9, 0xd1], "ground");
        assert_eq!(t.raised(), [0xce, 0xcb, 0xc2], "side");
        assert_eq!(t.bg(), [0xea, 0xe7, 0xe0], "fields");
        assert_eq!(t.ink(), [0x27, 0x25, 0x1f], "ink");
        // And the dock's red stays the dock's red.
        assert_eq!(resolve(&golem_choice(), true).c[0x08], [0xe0, 0x52, 0x52]);
    }

    #[test]
    fn every_built_in_palette_reads_and_reads_well() {
        // (Golem's palette is the fallback behind the box, not one of its tiles.)
        for (id, text) in BUILT_IN {
            let p = &parse(id, text).unwrap_or_else(|| panic!("{id} parses"));
            for (dark, c) in [(true, p.dark), (false, p.light)] {
                let t = Theme { c, ..plain(p, dark) };
                // The ground is on the look's side, and the ink reads on it.
                assert_eq!(rel_lum(t.bg()) < 0.18, dark, "{id} dark={dark}: ground on the right side");
                assert!(contrast(rel_lum(t.surface()), rel_lum(t.ink())) >= 4.5, "{id} dark={dark}: ink reads on the surface");
            }
        }
    }

    #[test]
    fn a_palette_without_a_light_variant_is_turned_over() {
        let text = BUILT_IN[0].1;
        let mut v: serde_json::Value = serde_json::from_str(text).unwrap();
        v.as_object_mut().unwrap().remove("light");
        let p = parse("x", &v.to_string()).unwrap();
        assert_eq!(p.light[0], p.dark[7]);
        assert_eq!(p.light[7], p.dark[0]);
        assert_eq!(p.light[0x0D], p.dark[0x0D]);
    }

    #[test]
    fn a_broken_palette_is_refused() {
        assert!(parse("x", r##"{"name":"x","dark":{"base00":"#000000"}}"##).is_none());
        assert!(parse("x", "not json").is_none());
        assert_eq!(parse_hex("#ffbe98"), Some([0xff, 0xbe, 0x98]));
        assert_eq!(parse_hex("ffbe9"), None);
        assert_eq!(slot("base0d"), Some(13));
        assert_eq!(slot("base10"), None);
    }

    #[test]
    fn the_files_carry_the_palette() {
        let t = resolve(&golem_choice(), true);
        let foot = foot_ini(&t);
        assert!(foot.contains("initial-color-theme=dark") && foot.contains("[colors-light]"));
        assert!(foot.contains("background=16171a"));
        assert!(gtk4_css(&t).contains("--accent-bg-color: #ffbe98;"));
        assert!(gtk4_css(&t).contains("@media (prefers-color-scheme: dark)"));
        assert!(hyprlock_conf(&t).contains("$golem_accent = rgb(255, 190, 152)"));
        let qt = qt_scheme(&t);
        assert_eq!(qt.lines().nth(1).unwrap().split(", ").count(), 22);
        let qt5 = qt5_scheme(&t);
        for l in qt5.lines().skip(1) {
            assert_eq!(l.split_once('=').unwrap().1.split(", ").count(), 21, "Qt 5's count: {l}");
        }
        assert_eq!(adwaita_accent([0xff, 0xbe, 0x98]), "orange");
        let ini = "[General]\nFirstRun=true\n\n[Appearance]\nFontScale=10\n";
        let with = with_obs_theme(ini).expect("no theme yet");
        assert!(with.contains("[Appearance]\nTheme=com.obsproject.System\nFontScale=10"));
        assert_eq!(with_obs_theme(&with), None, "a theme named is kept");
        assert!(with_obs_theme("[General]\nFirstRun=true\n").unwrap().ends_with("\n[Appearance]\nTheme=com.obsproject.System\n"));
        let g3 = gtk3_theme_css(&t, Path::new("/x/adw-gtk3-dark/gtk-3.0/gtk.css"));
        assert!(g3.contains("@import url(\"file:///x/adw-gtk3-dark/gtk-3.0/gtk.css\");"));
        assert!(g3.contains("@define-color window_bg_color #1d2026;") && g3.contains("@define-color accent_bg_color #ffbe98;"));
        assert_eq!(g3.matches("@define-color").count(), 35);
        // (Selected rows: the accent with its own ink, never adw-gtk3's white.)
        assert!(g3.contains(&format!("@define-color theme_selected_fg_color #{};", hex(t.accent_ink()))));
        let btop = btop_theme(&t);
        assert!(btop.contains("theme[main_bg]=\"#16171a\"") && btop.contains("theme[title]=\"#ffbe98\""));
        assert_eq!(btop.matches("theme[").count(), 10 + 5 + 27);
        let osc = foot_osc(&t);
        assert!(osc.contains("\x1b]11;#16171a\x1b\\"), "ground");
        // (Red as words — text-safe — the same in the live OSC as in foot.ini.)
        let red = hex(as_text(&t.c, t.c[8]));
        assert!(osc.contains(&format!("\x1b]4;1;#{red}\x1b\\")), "red, as words");
        assert!(foot_ini(&t).contains(&format!("regular1={red}")), "red, as the live OSC");
        assert_eq!(osc.matches("\x1b]4;").count(), 16);
        assert_eq!(adwaita_accent([0x8a, 0xb4, 0xf0]), "blue");
        // Text on the peach accent is the dark end.
        assert_eq!(t.accent_ink(), t.c[0]);
        let json: serde_json::Value = serde_json::from_str(&colors_json(&t, &t, &ColorChoice::default(), &["open.spotify.com".to_owned()])).unwrap();
        assert_eq!(json["accent"], "#ffbe98");
        assert_eq!(json["web"], false, "websites stay as made unless asked");
        assert_eq!(json["apps"], true);
        assert_eq!(json["webapp_hosts"][0], "open.spotify.com");
        assert_eq!(json["web_bg"], "#1d2026");
        let nvim = nvim_lua(&t);
        assert!(nvim.contains("base09 = '#ffbe98',") && nvim.contains("vim.o.background = 'dark'"));
    }
}

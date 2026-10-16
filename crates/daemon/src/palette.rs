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
    ("catppuccin", include_str!("../assets/palettes/catppuccin.json")),
    ("gruvbox", include_str!("../assets/palettes/gruvbox.json")),
    ("nord", include_str!("../assets/palettes/nord.json")),
];

/// The owner's colour choice, as saved in `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ColorChoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    #[serde(default = "yes")]
    pub apps: bool,
    /// (Once `dark_websites`, when it only worked in the dark look.)
    #[serde(default, alias = "dark_websites")]
    pub web: bool,
    /// Before 2026-10-15 the screen's colours were a switch of their own
    /// beside the palettes; read once, never written again.
    #[serde(default, skip_serializing)]
    pub dynamic: Option<bool>,
}

fn yes() -> bool {
    true
}

impl Default for ColorChoice {
    fn default() -> Self {
        Self { palette: None, apps: true, web: false, dynamic: None }
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
}

/// A palette file.
#[derive(Debug, Clone, Deserialize)]
struct PaletteFile {
    name: String,
    #[serde(default)]
    accent: Option<String>,
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
    /// The slot of its accent (8..=15).
    pub accent: usize,
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
    pub(crate) fn accent(&self) -> Rgb {
        self.c[self.accent_slot]
    }
    /// The ink that reads ON the accent: the palette's darkest or its
    /// brightest, whichever stands further from it.
    pub(crate) fn accent_ink(&self) -> Rgb {
        let a = rel_lum(self.accent());
        let (first, last) = (self.c[0x00], self.c[0x07]);
        if contrast(a, rel_lum(first)) >= contrast(a, rel_lum(last)) {
            first
        } else {
            last
        }
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
    Some(Palette { id: id.to_owned(), name: file.name, dark, light, accent })
}

/// The owner's own palettes' directory.
fn own_dir() -> PathBuf {
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

/// Every palette there is: Golem's first, then the owner's by name.
pub(crate) fn all() -> Vec<Palette> {
    let mut out: Vec<Palette> = BUILT_IN.iter().filter_map(|(id, text)| parse(id, text)).collect();
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
            if BUILT_IN.iter().any(|(b, _)| *b == id) {
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

/// The theme a choice gives in a look.
pub(crate) fn resolve(choice: &ColorChoice, dark: bool) -> Theme {
    let want = choice.palette_id();
    let p = if want == DYNAMIC_ID {
        dynamic_palette()
    } else {
        all().into_iter().find(|p| p.id == want)
    }
    .unwrap_or_else(golem);
    let accent_slot = p.accent;
    Theme {
        dynamic: want == DYNAMIC_ID,
        id: if want == DYNAMIC_ID { DYNAMIC_ID.to_owned() } else { p.id },
        name: p.name,
        dark,
        apps: choice.apps,
        web: choice.web,
        c: if dark { p.dark } else { p.light },
        accent_slot,
    }
}

fn golem() -> Palette {
    parse(GOLEM, BUILT_IN[0].1).expect("Golem's palette parses")
}

/// The makers' colours, in the same look as `t`: what the programs wear
/// when the palette is kept off them.
pub(crate) fn stock(t: &Theme) -> Theme {
    let p = parse("stock", STOCK).expect("the Stock palette parses");
    Theme { id: p.id, name: p.name, c: if t.dark { p.dark } else { p.light }, accent_slot: p.accent, ..t.clone() }
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
    Palette {
        id: DYNAMIC_ID.to_owned(),
        name: "Dynamic".to_owned(),
        dark: join(dark_greys, dark_hues),
        light: join(light_greys, light_hues),
        accent: accent_slot,
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
pub(crate) fn accent_srgb() -> [f32; 4] {
    srgb(current().accent())
}
pub(crate) fn danger_srgb() -> [f32; 4] {
    srgb(current().c[0x08])
}
pub(crate) fn warn_srgb() -> [f32; 4] {
    srgb(current().c[0x0A])
}

/// A colour of the palette as the renderer wants it: linear, opaque.
pub(crate) fn linear(c: Rgb) -> [f32; 4] {
    let f = |v: u8| crate::options::srgb_to_linear(f32::from(v) / 255.0);
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

fn rel_lum(c: Rgb) -> f32 {
    let l = linear(c);
    0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
}

fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// ------------------------------------------------------------ the apps

/// Write the theme out for everything outside the daemon and tell the
/// running apps — on a thread of its own: it runs programs.
pub(crate) fn publish(theme: Arc<Theme>, look_changed: bool) {
    let spawned = std::thread::Builder::new().name("palette".into()).spawn(move || {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
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
        }
        info!("palette: {} ({}), accent {}", theme.name, if theme.dark { "dark" } else { "light" }, slot_name(theme.accent_slot));
    });
    if let Err(e) = spawned {
        warn!("palette: no thread to write the theme ({e})");
    }
}

/// Whether two themes dress the programs alike: the same colours, accent and
/// look (the web switch and the name aside).
fn same_for_apps(a: &Theme, b: &Theme) -> bool {
    a.id == b.id && a.c == b.c && a.dark == b.dark && a.accent_slot == b.accent_slot
}

/// Only the file every reader shares (`colors.json`: Seam's web switches).
fn write_shared(dir: &Path, shell: &Theme) {
    let t = &for_apps(shell);
    crate::persist::write_text("theme", &dir.join("colors.json"), &colors_json(t, shell, &crate::webapps::app_hosts()));
}

/// What the programs wear: the theme, or the makers' colours when the
/// palette is kept off them (`apps` off).
pub(crate) fn for_apps(t: &Theme) -> Theme {
    if t.apps {
        t.clone()
    } else {
        stock(t)
    }
}

/// A palette by id, both its variants: the made one, the stock one, or one
/// of [`all`].
fn palette_of(id: &str) -> Option<Palette> {
    match id {
        DYNAMIC_ID => dynamic_palette(),
        "stock" => parse("stock", STOCK),
        _ => all().into_iter().find(|p| p.id == id),
    }
}

/// Every file of the theme, into `dir`. The lock screen is the shell's and
/// wears the palette always; the rest is the programs'.
pub(crate) fn write_all(dir: &Path, shell: &Theme) {
    let t = &for_apps(shell);
    let put = |name: &str, text: String| crate::persist::write_text("theme", &dir.join(name), &text);
    put("colors.json", colors_json(t, shell, &crate::webapps::app_hosts()));
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
fn colors_json(t: &Theme, shell: &Theme, webapp_hosts: &[String]) -> String {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), t.name.clone().into());
    m.insert("palette".into(), t.id.clone().into());
    m.insert("dark".into(), t.dark.into());
    m.insert("dynamic".into(), t.dynamic.into());
    m.insert("apps".into(), t.apps.into());
    m.insert("web".into(), t.web.into());
    // (What a Seam from before 2026-10-15 reads: it only darkened.)
    m.insert("dark_websites".into(), (t.web && t.dark).into());
    m.insert("web_bg".into(), format!("#{}", hex(shell.surface())).into());
    m.insert("web_ink".into(), format!("#{}", hex(shell.ink())).into());
    m.insert("webapp_hosts".into(), webapp_hosts.to_vec().into());
    for (i, c) in t.c.iter().enumerate() {
        m.insert(slot_name(i), format!("#{}", hex(*c)).into());
    }
    for (role, c) in [
        ("bg", t.bg()),
        ("surface", t.surface()),
        ("raised", t.raised()),
        ("border", t.border()),
        ("ink_dim", t.ink_dim()),
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
    let variant = |c: &[Rgb; 16]| {
        let accent = c[t.accent_slot];
        let h = |i: usize| hex(c[i]);
        format!(
            "background={bg}\nforeground={fg}\ncursor={bg} {fg}\nselection-background={sel}\nselection-foreground={fg}\n\
             urls={accent}\n\
             regular0={r0}\nregular1={r1}\nregular2={r2}\nregular3={r3}\nregular4={r4}\nregular5={r5}\nregular6={r6}\nregular7={r7}\n\
             bright0={b0}\nbright1={r1}\nbright2={r2}\nbright3={r3}\nbright4={r4}\nbright5={r5}\nbright6={r6}\nbright7={b7}\n",
            bg = h(0),
            fg = h(5),
            sel = h(2),
            accent = hex(accent),
            r0 = h(2),
            r1 = h(8),
            r2 = h(0xB),
            r3 = h(0xA),
            r4 = h(0xD),
            r5 = h(0xE),
            r6 = h(0xC),
            r7 = h(5),
            b0 = h(3),
            b7 = h(7),
        )
    };
    let (dark, light) = match pal {
        Some(p) => (p.dark, p.light),
        None => (t.c, t.c),
    };
    format!(
        "# Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change.\n\
         [main]\ninitial-color-theme={}\n\n[colors-dark]\n{}\n[colors-light]\n{}",
        t.name,
        if t.dark { "dark" } else { "light" },
        variant(&dark),
        variant(&light),
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
    let (dark, light) = match pal {
        Some(p) => (p.dark, p.light),
        None => (t.c, t.c),
    };
    let block = |c: &[Rgb; 16]| {
        let th = Theme { c: *c, ..t.clone() };
        let h = |x: Rgb| format!("#{}", hex(x));
        let vars = [
            ("accent-bg-color", h(th.accent())),
            ("accent-fg-color", h(th.accent_ink())),
            ("accent-color", h(th.accent())),
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
            ("destructive-bg-color", h(c[8])),
            ("destructive-color", h(c[8])),
            ("success-bg-color", h(c[0xB])),
            ("success-color", h(c[0xB])),
            ("warning-bg-color", h(c[0xA])),
            ("warning-color", h(c[0xA])),
            ("error-bg-color", h(c[8])),
            ("error-color", h(c[8])),
        ];
        vars.iter().map(|(k, v)| format!("  --{k}: {v};\n")).collect::<String>()
    };
    format!(
        "/* Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change. */\n\
         :root {{\n{}}}\n\n@media (prefers-color-scheme: dark) {{\n  :root {{\n{}  }}\n}}\n",
        t.name,
        block(&light),
        block(&dark).lines().map(|l| format!("  {l}\n")).collect::<String>(),
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
        ("accent_color", h(t.accent())),
        ("accent_bg_color", h(t.accent())),
        ("accent_fg_color", h(t.accent_ink())),
        ("destructive_color", h(c[8])),
        ("destructive_bg_color", h(c[8])),
        ("destructive_fg_color", ink(c[8])),
        ("error_color", h(c[8])),
        ("error_bg_color", h(c[8])),
        ("error_fg_color", ink(c[8])),
        ("success_color", h(c[0xB])),
        ("success_bg_color", h(c[0xB])),
        ("success_fg_color", ink(c[0xB])),
        ("warning_color", h(c[0xA])),
        ("warning_bg_color", h(c[0xA])),
        ("warning_fg_color", ink(c[0xA])),
    ] {
        out.push_str(&format!("@define-color {name} {col};\n"));
    }
    out
}

/// The palette's darkest or brightest, whichever reads on `bg`.
fn ink_for(t: &Theme, bg: Rgb) -> Rgb {
    let a = rel_lum(bg);
    let (first, last) = (t.c[0x00], t.c[0x07]);
    if contrast(a, rel_lum(first)) >= contrast(a, rel_lum(last)) {
        first
    } else {
        last
    }
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
    let dir = PathBuf::from(data_home).join("themes").join(next);
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
}

/// Qt (through qt5ct/qt6ct): a colour scheme, one colour per palette role
/// in `QPalette::ColorRole` order.
fn qt_scheme(t: &Theme) -> String {
    let c = &t.c;
    let a = |x: Rgb| format!("#ff{}", hex(x));
    let dim = |x: Rgb| format!("#80{}", hex(x));
    let roles = |text: Rgb, faint: bool| {
        let ink = if faint { dim(text) } else { a(text) };
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
            a(c[0xD]),            // Link
            a(c[0xE]),            // LinkVisited
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
    format!(
        "[Appearance]\ncolor_scheme_path={}\ncustom_palette=true\nicon_theme={}\nstandard_dialogs=default\nstyle=Fusion\n",
        scheme.display(),
        if t.dark { "Golem-Dark" } else { "Golem" },
    )
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
    let a = format!("#{}", hex(t.accent()));
    let mut out = format!("# Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change.\n", t.name);
    let mut put = |k: &str, v: &str| out.push_str(&format!("theme[{k}]=\"{v}\"\n"));
    put("main_bg", &h(0));
    put("main_fg", &h(5));
    put("title", &a);
    put("hi_fg", &a);
    put("selected_bg", &h(2));
    put("selected_fg", &h(7));
    put("inactive_fg", &h(3));
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
        put(&format!("{k}_start"), &h(s));
        put(&format!("{k}_mid"), &h(m));
        put(&format!("{k}_end"), &h(e));
    }
    out
}

/// Neovim (mini.base16, Golem's home layer loads this at each start).
fn nvim_lua(t: &Theme) -> String {
    let palette: String = t.c.iter().enumerate().map(|(i, c)| format!("    {} = '#{}',\n", slot_name(i), hex(*c))).collect();
    format!(
        "-- Golem's palette ({}), written by the dock. Do not edit: it is rewritten on every change.\n\
         vim.o.background = '{}'\n\
         require('mini.base16').setup({{\n  palette = {{\n{}  }},\n  use_cterm = true,\n}})\n\
         vim.g.colors_name = 'golem'\n",
        t.name,
        if t.dark { "dark" } else { "light" },
        palette,
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
    // btop reads its config again on SIGUSR2 (and with it, its theme).
    signal_all("btop", libc::SIGUSR2);
}

/// Send `sig` to every process of ours named `comm`.
fn signal_all(comm: &str, sig: libc::c_int) {
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
fn foot_osc(t: &Theme) -> String {
    let c = &t.c;
    let sixteen = [c[2], c[8], c[0xB], c[0xA], c[0xD], c[0xE], c[0xC], c[5], c[3], c[8], c[0xB], c[0xA], c[0xD], c[0xE], c[0xC], c[7]];
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
    if out(program, args).is_none() {
        debug!("palette: {program} {args:?} did not succeed");
    }
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
        for (k, v) in launch_vars() {
            crate::hypr::eval(&format!("hl.env('{k}', '{v}')"));
        }
        let theme = resolve(&choice(), crate::look::dark());
        // The Dynamic palette of a new wallpaper is being made: Golem's
        // stands in, and this comes back when it is ready.
        if theme.dynamic && dynamic_pending() {
            let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(250));
            let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut crate::App| {
                app.colors_apply(look_changed);
                calloop::timer::TimeoutAction::Drop
            });
        }
        let changed = set(theme);
        if changed || look_changed {
            publish(current(), look_changed);
        }
        if changed {
            self.reassert_window_border();
            self.draw_options();
            self.draw();
            self.filer_redraw_all();
        }
    }

    /// A choice in the "Colours" setting: save it, then put it in use.
    pub(crate) fn colors_pick(&mut self, key: &str, value: &str) {
        GolemSettings::change(|s| {
            let mut c = s.colors.clone().unwrap_or_default();
            match key {
                "colors-palette" => {
                    c.palette = Some(value.to_owned());
                    c.dynamic = None;
                }
                "colors-apps" => c.apps = value == "on",
                "colors-web" => c.web = value == "on",
                _ => return,
            }
            s.colors = Some(c);
            s.store();
        });
        self.colors_apply(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golem_choice() -> ColorChoice {
        ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }
    }

    #[test]
    fn the_old_choices_carry_over() {
        let read = |j: &str| serde_json::from_str::<ColorChoice>(j).unwrap();
        // Nothing chosen: the Dynamic palette, the programs wearing it, the web as made.
        let none = read("{}");
        assert_eq!((none.palette_id(), none.apps, none.web), (DYNAMIC_ID, true, false));
        // The old switch on (the screen's colours): Dynamic, whatever palette it named.
        assert_eq!(read(r#"{"dynamic":true,"palette":"nord"}"#).palette_id(), DYNAMIC_ID);
        // Off: the palette it named; dark websites on → colours on the web.
        let max = read(r#"{"dynamic":false,"palette":"fluor","dark_websites":true}"#);
        assert_eq!((max.palette_id(), max.web), ("fluor", true));
        // Written back: no `dynamic`, the palette by name.
        let back = serde_json::to_value(ColorChoice { palette: Some("nord".into()), ..max }).unwrap();
        assert_eq!(back, serde_json::json!({"palette": "nord", "apps": true, "web": true}));
    }

    #[test]
    fn a_web_switch_leaves_the_programs_alone() {
        let t = resolve(&ColorChoice { palette: Some(GOLEM.to_owned()), ..Default::default() }, true);
        assert!(same_for_apps(&t, &Theme { web: true, ..t.clone() }));
        assert!(!same_for_apps(&t, &Theme { dark: false, ..t.clone() }));
        assert!(!same_for_apps(&for_apps(&t), &for_apps(&Theme { apps: false, ..t.clone() })));
    }

    #[test]
    fn the_stock_colours_read_well() {
        let t = resolve(&golem_choice(), true);
        for dark in [true, false] {
            let s = stock(&Theme { dark, ..t.clone() });
            assert_eq!(s.id, "stock");
            assert_eq!(rel_lum(s.bg()) < 0.18, dark);
            assert!(contrast(rel_lum(s.surface()), rel_lum(s.ink())) >= 4.5);
        }
        assert_eq!(adwaita_accent(stock(&t).accent()), "blue", "libadwaita's own accent");
        let off = Theme { apps: false, ..t.clone() };
        assert_eq!(for_apps(&off).id, "stock");
        assert_eq!(for_apps(&t).id, GOLEM);
    }

    #[test]
    fn a_picture_gives_a_palette_of_its_own() {
        // A forest green picture with some grey: a green accent, greenish greys.
        let mut px = vec![[0x2e, 0x7d, 0x32]; 600];
        px.extend(vec![[0x80, 0x80, 0x80]; 400]);
        let p = from_pixels(&px);
        assert_eq!(p.accent, 0x0B, "green, in green's slot");
        for (dark, c) in [(true, p.dark), (false, p.light)] {
            let t = Theme { id: p.id.clone(), name: p.name.clone(), dark, dynamic: true, apps: true, web: false, c, accent_slot: p.accent };
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
        let all = all();
        for (id, _) in BUILT_IN {
            let p = all.iter().find(|p| p.id == *id).unwrap_or_else(|| panic!("{id} parses"));
            for (dark, c) in [(true, p.dark), (false, p.light)] {
                let t = Theme { id: p.id.clone(), name: p.name.clone(), dark, dynamic: false, apps: true, web: false, c, accent_slot: p.accent };
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
        assert_eq!(g3.matches("@define-color").count(), 33);
        let btop = btop_theme(&t);
        assert!(btop.contains("theme[main_bg]=\"#16171a\"") && btop.contains("theme[title]=\"#ffbe98\""));
        assert_eq!(btop.matches("theme[").count(), 10 + 5 + 27);
        let osc = foot_osc(&t);
        assert!(osc.contains("\x1b]11;#16171a\x1b\\"), "ground");
        assert!(osc.contains("\x1b]4;1;#e05252\x1b\\"), "red, as foot.ini");
        assert_eq!(osc.matches("\x1b]4;").count(), 16);
        assert_eq!(adwaita_accent([0x8a, 0xb4, 0xf0]), "blue");
        // Text on the peach accent is the dark end.
        assert_eq!(t.accent_ink(), t.c[0]);
        let json: serde_json::Value = serde_json::from_str(&colors_json(&t, &t, &["open.spotify.com".to_owned()])).unwrap();
        assert_eq!(json["accent"], "#ffbe98");
        assert_eq!(json["web"], false, "websites stay as made unless asked");
        assert_eq!(json["apps"], true);
        assert_eq!(json["webapp_hosts"][0], "open.spotify.com");
        assert_eq!(json["web_bg"], "#1d2026");
        let nvim = nvim_lua(&t);
        assert!(nvim.contains("base09 = '#ffbe98',") && nvim.contains("vim.o.background = 'dark'"));
    }
}

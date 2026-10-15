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
//! The owner's choice lives under `colors` in `~/.config/golem/settings.json`:
//!
//! - `dynamic` — on (the default): the shell keeps colouring itself from the
//!   screen, exactly as it always has (the banner always does); off: the
//!   dock, the boxes and the window borders wear the palette instead.
//! - `palette` — which one (absent: Golem's).
//! - `dark_websites` — off by default: websites stay as they are made. On:
//!   Seam darkens them (Dark Reader) while the look is dark — it reads this
//!   from `colors.json` (Golem's `seam/golem-chrome.js`, DARK WEBSITES).
//!
//! The ACCENT is the palette's own (its file names the slot) — never chosen
//! apart from it (Max, 2026-10-15: "the acent colour should come from the
//! pallete, not apart"). A palette that wants another accent is another
//! palette.
//!
//! The dark/light choice stays the "Dark mode" setting's; it picks the
//! palette's variant.
//!
//! [`resolve`] makes the [`Theme`] — what is in use now — and [`publish`]
//! writes it out for everything outside the daemon, in
//! `~/.local/state/golem/theme/` (`colors.json` for anyone; `foot.ini`,
//! `hyprlock.conf`, `gtk4.css`, `gtk3.css`, `qt.conf`, `nvim.lua` for the apps Golem's
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
    #[serde(default = "yes")]
    pub dynamic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dark_websites: bool,
}

fn yes() -> bool {
    true
}

impl Default for ColorChoice {
    fn default() -> Self {
        Self { dynamic: true, palette: None, dark_websites: false }
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
    pub dynamic: bool,
    pub dark_websites: bool,
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
    let palettes = all();
    let want = choice.palette.as_deref().unwrap_or(GOLEM);
    let p = palettes
        .iter()
        .find(|p| p.id == want)
        .or_else(|| palettes.iter().find(|p| p.id == GOLEM))
        .cloned()
        .unwrap_or_else(|| parse(GOLEM, BUILT_IN[0].1).expect("Golem's palette parses"));
    let accent_slot = p.accent;
    Theme {
        id: p.id,
        name: p.name,
        dark,
        dynamic: choice.dynamic,
        dark_websites: choice.dark_websites,
        c: if dark { p.dark } else { p.light },
        accent_slot,
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
    Arc::new(resolve(&ColorChoice::default(), true))
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
        write_all(&theme_dir(), &theme);
        poke(&theme, look_changed);
        info!("palette: {} ({}), accent {}", theme.name, if theme.dark { "dark" } else { "light" }, slot_name(theme.accent_slot));
    });
    if let Err(e) = spawned {
        warn!("palette: no thread to write the theme ({e})");
    }
}

/// Every file of the theme, into `dir`.
pub(crate) fn write_all(dir: &Path, t: &Theme) {
    let put = |name: &str, text: String| crate::persist::write_text("theme", &dir.join(name), &text);
    put("colors.json", colors_json(t));
    put("foot.ini", foot_ini(t));
    put("hyprlock.conf", hyprlock_conf(t));
    put("gtk4.css", gtk4_css(t));
    put("gtk3.css", gtk3_css(t));
    put("qt.conf", qt_scheme(t));
    put("nvim.lua", nvim_lua(t));
    for app in ["qt5ct", "qt6ct"] {
        let conf = config_home().join(app).join(format!("{app}.conf"));
        // Ours to write: absent, or already pointing at the theme. One
        // the owner (or a module) set up otherwise is left alone.
        let ours = match std::fs::symlink_metadata(&conf) {
            Err(_) => true,
            Ok(m) if m.file_type().is_symlink() => false,
            Ok(_) => std::fs::read_to_string(&conf).is_ok_and(|s| s.contains("golem/theme/qt.conf")),
        };
        if ours {
            crate::persist::write_text("theme", &conf, &qtct_conf(&dir.join("qt.conf"), t));
        }
    }
}

/// For anyone who wants the colours (the browser's chrome, scripts): the
/// sixteen, the roles, the accent.
fn colors_json(t: &Theme) -> String {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), t.name.clone().into());
    m.insert("palette".into(), t.id.clone().into());
    m.insert("dark".into(), t.dark.into());
    m.insert("dynamic".into(), t.dynamic.into());
    m.insert("dark_websites".into(), t.dark_websites.into());
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
    let p = all();
    let pal = p.iter().find(|p| p.id == t.id);
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
    let pal = all().into_iter().find(|p| p.id == t.id);
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

/// GTK 3 (Adwaita): the named colours it lets a user override — the look
/// in use only (GTK 3 has no colour-scheme query; its apps restart anyway).
fn gtk3_css(t: &Theme) -> String {
    let h = |x: Rgb| format!("#{}", hex(x));
    format!(
        "/* Golem's palette ({}), written by the dock. */\n\
         @define-color theme_bg_color {};\n@define-color theme_fg_color {};\n\
         @define-color theme_base_color {};\n@define-color theme_text_color {};\n\
         @define-color theme_selected_bg_color {};\n@define-color theme_selected_fg_color {};\n\
         @define-color accent_color {};\n@define-color accent_bg_color {};\n@define-color accent_fg_color {};\n",
        t.name,
        h(t.surface()),
        h(t.ink()),
        h(t.bg()),
        h(t.ink()),
        h(t.accent()),
        h(t.accent_ink()),
        h(t.accent()),
        h(t.accent()),
        h(t.accent_ink()),
    )
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

fn qtct_conf(scheme: &Path, t: &Theme) -> String {
    format!(
        "[Appearance]\ncolor_scheme_path={}\ncustom_palette=true\nicon_theme={}\nstandard_dialogs=default\nstyle=Fusion\n",
        scheme.display(),
        if t.dark { "Golem-Dark" } else { "Golem" },
    )
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

fn run(program: &str, args: &[&str]) {
    match std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) if s.success() => {}
        other => debug!("palette: {program} {args:?}: {other:?}"),
    }
}


// --------------------------------------------------------------- the shell

impl crate::App {
    /// Read the colour choice again and put it in use: the shell redraws in
    /// it and the apps get it. At start, on a pick in the panel, and when
    /// the look turns dark or light.
    pub(crate) fn colors_apply(&mut self, look_changed: bool) {
        let theme = resolve(&choice(), crate::look::dark());
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
                "colors-dynamic" => c.dynamic = value == "on",
                "colors-palette" => c.palette = (value != GOLEM).then(|| value.to_owned()),
                "colors-websites" => c.dark_websites = value == "on",
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

    #[test]
    fn golems_palette_reads_and_resolves() {
        let p = parse(GOLEM, BUILT_IN[0].1).expect("parses");
        assert_eq!(p.name, "Golem");
        assert_eq!(p.accent, 0x09);
        assert_eq!(p.dark[0x09], [0xff, 0xbe, 0x98], "the desktop's peach");
        let dark = resolve(&ColorChoice::default(), true);
        assert!(dark.dynamic && dark.dark);
        assert_eq!(dark.accent(), [0xff, 0xbe, 0x98]);
        assert!(rel_lum(dark.bg()) < rel_lum(dark.ink()), "dark: light ink on a dark ground");
        let light = resolve(&ColorChoice { dynamic: false, palette: None, dark_websites: false }, false);
        assert!(!light.dynamic);
        assert_eq!(light.accent_slot, 0x09, "the palette's own accent, always");
        // An old settings file that still names an accent of its own: ignored.
        let old: ColorChoice = serde_json::from_str(r#"{"dynamic":false,"accent":"base0D"}"#).unwrap();
        assert_eq!(resolve(&old, true).accent_slot, 0x09);
        assert!(rel_lum(light.bg()) > rel_lum(light.ink()), "light: dark ink on a light ground");
    }

    #[test]
    fn golems_light_is_the_files_window_max_tuned() {
        // 2026-10-14: Files' light look, tuned by eye — the palette carries it now.
        let t = resolve(&ColorChoice::default(), false);
        assert_eq!(t.surface(), [0xdc, 0xd9, 0xd1], "ground");
        assert_eq!(t.raised(), [0xce, 0xcb, 0xc2], "side");
        assert_eq!(t.bg(), [0xea, 0xe7, 0xe0], "fields");
        assert_eq!(t.ink(), [0x27, 0x25, 0x1f], "ink");
        // And the dock's red stays the dock's red.
        assert_eq!(resolve(&ColorChoice::default(), true).c[0x08], [0xe0, 0x52, 0x52]);
    }

    #[test]
    fn every_built_in_palette_reads_and_reads_well() {
        let all = all();
        for (id, _) in BUILT_IN {
            let p = all.iter().find(|p| p.id == *id).unwrap_or_else(|| panic!("{id} parses"));
            for (dark, c) in [(true, p.dark), (false, p.light)] {
                let t = Theme { id: p.id.clone(), name: p.name.clone(), dark, dynamic: false, dark_websites: false, c, accent_slot: p.accent };
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
        let t = resolve(&ColorChoice::default(), true);
        let foot = foot_ini(&t);
        assert!(foot.contains("initial-color-theme=dark") && foot.contains("[colors-light]"));
        assert!(foot.contains("background=16171a"));
        assert!(gtk4_css(&t).contains("--accent-bg-color: #ffbe98;"));
        assert!(gtk4_css(&t).contains("@media (prefers-color-scheme: dark)"));
        assert!(hyprlock_conf(&t).contains("$golem_accent = rgb(255, 190, 152)"));
        let qt = qt_scheme(&t);
        assert_eq!(qt.lines().nth(1).unwrap().split(", ").count(), 22);
        assert_eq!(adwaita_accent([0xff, 0xbe, 0x98]), "orange");
        let osc = foot_osc(&t);
        assert!(osc.contains("\x1b]11;#16171a\x1b\\"), "ground");
        assert!(osc.contains("\x1b]4;1;#e05252\x1b\\"), "red, as foot.ini");
        assert_eq!(osc.matches("\x1b]4;").count(), 16);
        assert_eq!(adwaita_accent([0x8a, 0xb4, 0xf0]), "blue");
        // Text on the peach accent is the dark end.
        assert_eq!(t.accent_ink(), t.c[0]);
        let json: serde_json::Value = serde_json::from_str(&colors_json(&t)).unwrap();
        assert_eq!(json["accent"], "#ffbe98");
        assert_eq!(json["dark_websites"], false, "websites stay as made unless asked");
        let nvim = nvim_lua(&t);
        assert!(nvim.contains("base09 = '#ffbe98',") && nvim.contains("vim.o.background = 'dark'"));
    }
}

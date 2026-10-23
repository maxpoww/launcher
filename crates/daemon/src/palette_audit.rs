//! THE LEGIBILITY AUDIT (Max, 2026-10-23: "no podemos tener estos problemas
//! en ningún lado… lo importante acá es la experiencia de los usuarios"):
//! every pair of words over a ground that Golem paints, or writes into a
//! program, AS IT IS SEEN — in every palette, both looks, across the
//! Lighting, the Saturation and the Palette slider. A report for now:
//!
//! `cargo test -p waverunner-daemon legibility_audit -- --ignored --nocapture`
//! (`AUDIT_OUT=<file>` writes the whole table there too). Once everything
//! passes it becomes the floor the build asserts.
//!
//! "As seen" means, where it applies:
//! - THROUGH SEE-THROUGH GROUNDS. The dock and the boxes are layers the
//!   compositor lays over whatever is behind: their premultiplied colour is
//!   stored as sRGB, then `+ behind × (1 − a)` (measured on the dock,
//!   2026-10-23: 255,255,248 over a 245,238,231 picture). Judged over a
//!   black, a grey and a white backdrop.
//! - FADED WORDS: the lists' dim, the section titles, placeholders, disabled.
//! - THE LABEL COLOUR AS THE TEXT RENDERER TAKES IT: sRGB bytes. A linear
//!   colour handed to it shows as its numbers read as sRGB (darker).
//! - A WINDOW OUT OF FOCUS: Hyprland's `dim_inactive` lays black at 0.3 over
//!   it (`hyprland.lua`).
//!
//! Floors: 4.5 for words (WCAG AA), 3.0 for glyphs, large or bold titles,
//! placeholders, disabled and line numbers.

use super::*;
use crate::options::{linear_to_srgb, srgb_to_linear};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

const TEXT: f32 = 4.5;
const LARGE: f32 = 3.0;
/// `decoration.dim_strength` in hyprland.lua.
const DIM_INACTIVE: f32 = 0.3;
/// The dock's glass as the theme gives it (`core/config.rs`: #05070980).
const DOCK_ALPHA: f32 = 0.502;

/// sRGB, 0..1.
type S = [f32; 3];

fn s(c: Rgb) -> S {
    [0, 1, 2].map(|i| f32::from(c[i]) / 255.0)
}
fn lum(c: S) -> f32 {
    let l = c.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
    0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
}
fn ratio(a: S, b: S) -> f32 {
    contrast(lum(a), lum(b))
}
/// `fg` at `a` over `bg`, blended in sRGB (as GTK, Qt and the compositor do).
fn over(fg: S, a: f32, bg: S) -> S {
    [0, 1, 2].map(|i| fg[i] * a + bg[i] * (1.0 - a))
}
/// A window out of focus: black at `DIM_INACTIVE` over it.
fn dimmed(c: S) -> S {
    c.map(|v| v * (1.0 - DIM_INACTIVE))
}
fn hexs(c: S) -> String {
    c.iter().map(|v| format!("{:02x}", (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect()
}
fn parse_hex(h: &str) -> Option<S> {
    let h = h.trim().trim_start_matches('#');
    let h = if h.len() == 8 { &h[2..] } else { h };
    let v = u32::from_str_radix(h.get(..6)?, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8].map(|b| f32::from(b) / 255.0))
}
/// Qt's `#aarrggbb`: the colour and its alpha.
fn parse_argb(h: &str) -> Option<(S, f32)> {
    let h = h.trim().trim_start_matches('#');
    let a = u8::from_str_radix(h.get(..2)?, 16).ok()?;
    Some((parse_hex(h.get(2..)?)?, f32::from(a) / 255.0))
}
fn lin3(c: [f32; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]]
}
fn lin_of(c: Rgb) -> [f32; 3] {
    lin3(linear(c))
}

/// One of OUR layer's pixels as the screen shows it: a fill (linear) at `a`,
/// words at `ta` on it, over `behind` (sRGB). `words` are the renderer's
/// bytes (sRGB as given). Returns (words, ground) as seen.
fn layer(fill: [f32; 3], a: f32, words: S, ta: f32, behind: S) -> (S, S) {
    let ground = [0, 1, 2].map(|i| (linear_to_srgb(fill[i] * a) + behind[i] * (1.0 - a)).min(1.0));
    let w = words.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
    let pa = ta + a * (1.0 - ta);
    let seen = [0, 1, 2].map(|i| (linear_to_srgb(w[i] * ta + fill[i] * a * (1.0 - ta)) + behind[i] * (1.0 - pa)).min(1.0));
    (seen, ground)
}

const BEHIND: [(&str, S); 3] = [("black", [0.0; 3]), ("grey", [0.5; 3]), ("white", [1.0; 3])];

#[derive(Default)]
struct Stat {
    cases: usize,
    fails: usize,
    /// …of them, the palette as designed (Lighting 50, Saturation 50, as made).
    design_cases: usize,
    design_fails: usize,
    pals: BTreeSet<String>,
    worst: Option<f32>,
    ex: String,
}

#[derive(Default)]
struct Audit {
    rules: BTreeMap<(String, String), Stat>,
    pal: String,
    tag: String,
    design: bool,
}

impl Audit {
    fn check(&mut self, group: &str, rule: &str, fg: S, bg: S, floor: f32) {
        let r = ratio(fg, bg);
        let st = self.rules.entry((group.to_owned(), format!("{rule} [{floor}]"))).or_default();
        st.cases += 1;
        st.design_cases += usize::from(self.design);
        if r < floor {
            st.fails += 1;
            st.design_fails += usize::from(self.design);
            st.pals.insert(self.pal.clone());
            if st.worst.is_none_or(|w| r < w) {
                st.worst = Some(r);
                st.ex = format!("{} — #{} on #{}", self.tag, hexs(fg), hexs(bg));
            }
        }
    }
    /// An app's pair, and the same with its window out of focus.
    fn app(&mut self, group: &str, rule: &str, fg: S, bg: S, floor: f32) {
        self.check(group, rule, fg, bg, floor);
        self.check(&format!("{group} · out of focus"), rule, dimmed(fg), dimmed(bg), floor);
    }
}

/// `name → colour` from lines `prefix name value;`-ish, by a splitter.
fn colours(text: &str, split: impl Fn(&str) -> Option<(String, String)>) -> BTreeMap<String, S> {
    text.lines().filter_map(split).filter_map(|(k, v)| parse_hex(&v).map(|c| (k, c))).collect()
}

fn audit_apps(a: &mut Audit, apps: &Theme) {
    let c = |i: usize| s(apps.c[i]);
    // ---- GTK 3 (adw-gtk3 + our @define-colors)
    let g3 = colours(&gtk3_theme_css(apps, std::path::Path::new("/adw-gtk3")), |l| {
        let r = l.strip_prefix("@define-color ")?.trim_end_matches(';');
        let (k, v) = r.split_once(' ')?;
        Some((k.to_owned(), v.to_owned()))
    });
    let get3 = |k: &str| g3.get(k).copied().unwrap_or_else(|| panic!("gtk3: no {k}"));
    for w in ["window", "view", "headerbar", "sidebar", "card", "dialog", "popover", "thumbnail", "accent", "destructive", "error", "success", "warning"] {
        a.app("Apps · GTK 3", &format!("{w}_fg on {w}_bg"), get3(&format!("{w}_fg_color")), get3(&format!("{w}_bg_color")), TEXT);
    }
    a.app("Apps · GTK 3", "selected fg on selected bg (Seam's chosen suggestion)", get3("theme_selected_fg_color"), get3("theme_selected_bg_color"), TEXT);
    for x in ["accent", "destructive", "error", "success", "warning"] {
        for g in ["window", "view", "card"] {
            a.app("Apps · GTK 3", &format!("{x}_color as words on {g}_bg"), get3(&format!("{x}_color")), get3(&format!("{g}_bg_color")), TEXT);
        }
    }
    // ---- GTK 4 / libadwaita: the block of the look; the -fg-colors we do
    // not write are libadwaita's own (white; warning black at .8).
    let css = gtk4_css(apps);
    let (light, dark) = css.split_once("@media").unwrap_or((&css, ""));
    let g4 = colours(if apps.dark { dark } else { light }, |l| {
        let r = l.trim().strip_prefix("--")?.trim_end_matches(';');
        let (k, v) = r.split_once(':')?;
        Some((k.trim().to_owned(), v.trim().to_owned()))
    });
    let get4 = |k: &str| g4.get(k).copied().unwrap_or_else(|| panic!("gtk4: no {k}"));
    for w in ["window", "view", "headerbar", "sidebar", "secondary-sidebar", "card", "dialog", "popover", "thumbnail", "accent"] {
        a.app("Apps · GTK 4", &format!("{w}-fg on {w}-bg"), get4(&format!("{w}-fg-color")), get4(&format!("{w}-bg-color")), TEXT);
    }
    for (x, fg) in [("destructive", None), ("success", None), ("error", None), ("warning", Some(0.8))] {
        let bg = get4(&format!("{x}-bg-color"));
        let fg = match g4.get(&format!("{x}-fg-color")) {
            Some(f) => *f,
            None => fg.map_or([1.0; 3], |al| over([0.0; 3], al, bg)),
        };
        a.app("Apps · GTK 4", &format!("{x} button words (libadwaita's fg) on {x}-bg"), fg, bg, TEXT);
    }
    for x in ["accent", "destructive", "error", "success", "warning"] {
        for g in ["window", "view", "card"] {
            a.app("Apps · GTK 4", &format!("{x}-color as words on {g}-bg"), get4(&format!("{x}-color")), get4(&format!("{g}-bg-color")), TEXT);
        }
    }
    // ---- Qt (qt5ct/qt6ct): roles in order; disabled words at their alpha.
    let qt = qt_scheme(apps);
    let roles = |key: &str| -> Vec<(S, f32)> {
        let line = qt.lines().find(|l| l.starts_with(key)).unwrap_or_else(|| panic!("qt: no {key}"));
        line.split_once('=').unwrap().1.split(", ").map(|v| parse_argb(v).unwrap()).collect()
    };
    let (act, dis) = (roles("active_colors"), roles("disabled_colors"));
    let solid = |r: &[(S, f32)], i: usize, bg: S| over(r[i].0, r[i].1, bg);
    const WT: usize = 0;
    const BUTTON: usize = 1;
    const TX: usize = 6;
    const BT: usize = 8;
    const BASE: usize = 9;
    const WINDOW: usize = 10;
    const HI: usize = 12;
    const HT: usize = 13;
    const LINK: usize = 14;
    const VISITED: usize = 15;
    const ALT: usize = 16;
    const TIPB: usize = 18;
    const TIPT: usize = 19;
    const PH: usize = 20;
    for (rule, f, b, floor) in [
        ("WindowText on Window", WT, WINDOW, TEXT),
        ("Text on Base", TX, BASE, TEXT),
        ("Text on AlternateBase", TX, ALT, TEXT),
        ("ButtonText on Button", BT, BUTTON, TEXT),
        ("HighlightedText on Highlight", HT, HI, TEXT),
        ("Link on Base", LINK, BASE, TEXT),
        ("LinkVisited on Base", VISITED, BASE, TEXT),
        ("ToolTipText on ToolTipBase", TIPT, TIPB, TEXT),
        ("PlaceholderText on Base", PH, BASE, LARGE),
    ] {
        let bg = act[b].0;
        a.app("Apps · Qt", rule, solid(&act, f, bg), bg, floor);
    }
    for (rule, f, b) in [("disabled WindowText on Window", WT, WINDOW), ("disabled Text on Base", TX, BASE), ("disabled ButtonText on Button", BT, BUTTON)] {
        let bg = dis[b].0;
        a.app("Apps · Qt", rule, solid(&dis, f, bg), bg, LARGE);
    }
    // ---- KDE (kdeglobals): every set's words on its two grounds.
    for (section, keys) in crate::app_themes::kde_scheme(apps) {
        let Some(set) = section.strip_prefix("Colors:") else { continue };
        let get = |k: &str| {
            keys.iter().find(|(n, _)| n == k).and_then(|(_, v)| {
                let p: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect();
                (p.len() >= 3).then(|| [p[0] / 255.0, p[1] / 255.0, p[2] / 255.0])
            })
        };
        for bgk in ["BackgroundNormal", "BackgroundAlternate"] {
            let Some(bg) = get(bgk) else { continue };
            for (fk, floor) in [
                ("ForegroundNormal", TEXT),
                ("ForegroundActive", TEXT),
                ("ForegroundLink", TEXT),
                ("ForegroundVisited", TEXT),
                ("ForegroundNegative", TEXT),
                ("ForegroundNeutral", TEXT),
                ("ForegroundPositive", TEXT),
                ("ForegroundInactive", LARGE),
            ] {
                if let Some(fg) = get(fk) {
                    a.app("Apps · KDE", &format!("{set}: {fk} on {bgk}"), fg, bg, floor);
                }
            }
        }
    }
    // ---- foot: the variant of the look.
    let ini = foot_ini(apps);
    let sect = if apps.dark { "[colors-dark]" } else { "[colors-light]" };
    let body = ini.split(sect).nth(1).unwrap_or("").split("\n[").next().unwrap_or("");
    let ft = colours(body, |l| {
        let (k, v) = l.split_once('=')?;
        Some((k.to_owned(), v.split_whitespace().last()?.to_owned()))
    });
    let f = |k: &str| ft.get(k).copied().unwrap_or_else(|| panic!("foot: no {k}"));
    let bg = f("background");
    a.app("Apps · terminal", "foreground on background", f("foreground"), bg, TEXT);
    a.app("Apps · terminal", "selection-foreground on selection-background", f("selection-foreground"), f("selection-background"), TEXT);
    a.app("Apps · terminal", "urls on background", f("urls"), bg, TEXT);
    let cursor = body.lines().find_map(|l| l.strip_prefix("cursor=")).unwrap_or("");
    let cur: Vec<S> = cursor.split_whitespace().filter_map(parse_hex).collect();
    if cur.len() == 2 {
        a.app("Apps · terminal", "the letter under the cursor", cur[0], cur[1], TEXT);
    }
    for i in 1..8 {
        a.app("Apps · terminal", &format!("regular{i} on background"), f(&format!("regular{i}")), bg, TEXT);
        a.app("Apps · terminal", &format!("bright{i} on background"), f(&format!("bright{i}")), bg, TEXT);
    }
    a.app("Apps · terminal", "bright0 (dim grey) on background", f("bright0"), bg, LARGE);
    a.app("Apps · terminal", "regular0 (black) on background", f("regular0"), bg, LARGE);
    // yazi, by foot's names (Golem's home.nix).
    let (black, red, green, yellow, blue, magenta) = (f("regular0"), f("regular1"), f("regular2"), f("regular3"), f("regular4"), f("regular5"));
    for (rule, fg, b) in [
        ("hovered: black on yellow", black, yellow),
        ("selected: black on green", black, green),
        ("mode normal: black on blue", black, blue),
        ("mode unset: black on magenta", black, magenta),
        ("folders, cwd: yellow on background", yellow, bg),
        ("archives, write: red on background", red, bg),
    ] {
        a.app("Apps · yazi", rule, fg, b, TEXT);
    }
    a.app("Apps · yazi", "border: darkgray on background", f("bright0"), bg, LARGE);
    // ---- Neovim (mini.base16: the palette's slots as it maps them).
    for (rule, fg, b, floor) in [
        ("Normal c5 on c0", 5, 0, TEXT),
        ("NormalFloat, Pmenu c5 on c1", 5, 1, TEXT),
        ("PmenuSel c1 on c5", 1, 5, TEXT),
        ("Comment c3 on c0", 3, 0, TEXT),
        ("LineNr, SignColumn, TabLine, StatusLineNC, Folded c3 on c1", 3, 1, LARGE),
        ("CursorLineNr c4 on c1", 4, 1, TEXT),
        ("StatusLine c4 on c2", 4, 2, TEXT),
        ("Search, Substitute c1 on cA", 1, 0xA, TEXT),
        ("IncSearch, CurSearch c1 on c9", 1, 9, TEXT),
        ("WildMenu c8 on cA", 8, 0xA, TEXT),
        ("Todo cA on c1", 0xA, 1, TEXT),
        ("TabLineSel, DiffAdd cB on c1", 0xB, 1, TEXT),
        ("DiffDelete c8 on c1", 8, 1, TEXT),
        ("DiffChange cE on c1", 0xE, 1, TEXT),
        ("DiffText cD on c1", 0xD, 1, TEXT),
    ] {
        a.app("Apps · Neovim", rule, c(fg), c(b), floor);
    }
    for h in 8..16 {
        a.app("Apps · Neovim", &format!("syntax c{h:X} on c0"), c(h), c(0), TEXT);
        a.app("Apps · Neovim", &format!("syntax c{h:X} on Visual c2"), c(h), c(2), LARGE);
    }
    // ---- Text Editor (GtkSourceView), from the scheme as written.
    let xml = sourceview_scheme(apps, "golem");
    let attr = |l: &str, k: &str| l.split(&format!("{k}=\"")).nth(1).and_then(|r| r.split('"').next()).and_then(parse_hex);
    let mut styles: BTreeMap<String, (Option<S>, Option<S>)> = BTreeMap::new();
    for l in xml.lines() {
        if let Some(name) = l.split("name=\"").nth(1).and_then(|r| r.split('"').next()) {
            if l.trim_start().starts_with("<style ") {
                styles.insert(name.to_owned(), (attr(l, "foreground"), attr(l, "background")));
            }
        }
    }
    let text_bg = styles.get("text").and_then(|t| t.1).unwrap_or(c(0));
    let line_bg = styles.get("current-line").and_then(|t| t.1).unwrap_or(c(1));
    let text_fg = styles.get("text").and_then(|t| t.0).unwrap_or(c(5));
    for (name, (fg, bg)) in &styles {
        let floor = if name == "line-numbers" { LARGE } else { TEXT };
        match (fg, bg) {
            (Some(fg), Some(bg)) => a.app("Apps · Text Editor", name, *fg, *bg, floor),
            (Some(fg), None) if name != "cursor" => {
                a.app("Apps · Text Editor", &format!("{name} on the page"), *fg, text_bg, floor);
                a.app("Apps · Text Editor", &format!("{name} on the current line"), *fg, line_bg, floor);
            }
            (None, Some(bg)) if name == "search-match" => a.app("Apps · Text Editor", "words on a search match", text_fg, *bg, TEXT),
            _ => {}
        }
    }
    // ---- btop
    let bt = colours(&btop_theme(apps), |l| {
        let r = l.strip_prefix("theme[")?;
        let (k, v) = r.split_once("]=")?;
        Some((k.to_owned(), v.trim_matches('"').to_owned()))
    });
    let b = |k: &str| bt.get(k).copied().unwrap_or_else(|| panic!("btop: no {k}"));
    let main = b("main_bg");
    for (k, floor) in [("main_fg", TEXT), ("title", TEXT), ("hi_fg", TEXT), ("proc_misc", TEXT), ("graph_text", TEXT), ("inactive_fg", LARGE)] {
        a.app("Apps · btop", &format!("{k} on main_bg"), b(k), main, floor);
    }
    a.app("Apps · btop", "selected_fg on selected_bg", b("selected_fg"), b("selected_bg"), TEXT);
    for (k, v) in &bt {
        if k.ends_with("_start") || k.ends_with("_mid") || k.ends_with("_end") {
            a.app("Apps · btop", "graph colours on main_bg", *v, main, LARGE);
            a.app("Apps · btop", "graph colours on meter_bg", *v, b("meter_bg"), LARGE);
        }
    }
    // ---- Seam's own chrome (golem-chrome.js: base01 bar, base05 words).
    let (c0, c1, c4, c5) = (c(0), c(1), c(4), c(5));
    let mix = |x: S, y: S, k: f32| over(y, k, x);
    for (rule, fg, bg, floor) in [
        ("overview words c5 on the bar c1", c5, c1, TEXT),
        ("overview muted c4 on c1", c4, c1, TEXT),
        ("overview toolbar icons: fixed white on c1", [1.0; 3], c1, LARGE),
        ("tab card words on mix(c1, c5, .07)", c5, mix(c1, c5, 0.07), TEXT),
        ("tab card words, hovered, on mix(c1, c5, .13)", c5, mix(c1, c5, 0.13), TEXT),
        ("footer words on mix(c1, c0, .5)", c5, mix(c1, c0, 0.5), TEXT),
        ("letter badge c5 on c1 + white .10", c5, over([1.0; 3], 0.10, c1), TEXT),
    ] {
        a.app("Seam", rule, fg, bg, floor);
    }
}

fn audit_web(a: &mut Audit, apps: &Theme, shell: &Theme, choice: &ColorChoice) {
    let j: serde_json::Value = serde_json::from_str(&colors_json(apps, shell, choice, &[])).unwrap_or_default();
    let g = |k: &str| j[k].as_str().and_then(parse_hex);
    if let (Some(ink), Some(bg)) = (g("web_ink"), g("web_bg")) {
        a.app("Seam · Dark Reader", "pages: web_ink on web_bg", ink, bg, TEXT);
    }
    if let (Some(ink), Some(bg)) = (g("apps_ink"), g("apps_bg")) {
        a.app("Seam · Dark Reader", "webapps: apps_ink on apps_bg", ink, bg, TEXT);
    }
}

/// What Golem paints itself, in the look of `dark`, the shell at `shell`.
fn audit_shell(a: &mut Audit, choice: &ColorChoice, shell: &Theme, apps: &Theme, dark: bool) {
    let light = choice.shell_light.unwrap_or(50);
    // The lock screen: solid, the shell's theme.
    let (bg, raised, ink) = (s(shell.bg()), s(shell.raised()), s(shell.ink()));
    a.check("Lock screen", "clock: ink on bg", ink, bg, TEXT);
    a.check("Lock screen", "typed: ink on the field", ink, raised, TEXT);
    a.check("Lock screen", "ring: accent on bg", s(shell.accent()), bg, LARGE);
    // Golem Files: a solid window; its words handed to the renderer as the
    // linear colour.
    let (ground, words) = (lin_of(apps.c[1]), lin_of(apps.c[5]));
    let (w, g) = layer(ground, 1.0, words, 1.0, [0.0; 3]);
    a.check("Files", "names (as rendered) on the window", w, g, TEXT);
    let (w, g) = layer(ground, 1.0, s(apps.c[5]), 1.0, [0.0; 3]);
    a.check("Files", "names (if given as sRGB) on the window", w, g, TEXT);
    let (w, g) = layer(ground, 1.0, words, 0.5, [0.0; 3]);
    a.check("Files", "dim words at .5 (as rendered)", w, g, LARGE);
    a.check("Files", "rename: white on the accent", [1.0; 3], s(apps.accent()), TEXT);
    // The shell wearing the palette: the Dark look (the Light one reads the
    // screen: `audit_screen_glass`).
    if !dark {
        return;
    }
    let Some((surface, ink_lin)) = shell_plan_look(choice, true).pair(light) else { return };
    let (surface, ink_lin) = (lin3(surface), lin3(ink_lin));
    let bright = 0.2126 * surface[0] + 0.7152 * surface[1] + 0.0722 * surface[2] > 0.179;
    // `Backdrop::surface_wash`: the night light's dim on a dark ground.
    let fill = if bright { surface.map(|v| v * 0.9) } else { surface.map(|v| v * (1.0 - crate::options::NIGHT_DIM)) };
    let ink_bytes: S = ink_lin; // (handed over linear)
    let ink_srgb: S = ink_lin.map(linear_to_srgb);
    let list_dim = if ink_lin.iter().sum::<f32>() < 1.5 { crate::options::LIST_DIM_LIGHT } else { crate::options::LIST_DIM };
    for (bn, behind) in BEHIND {
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, ink_bytes, 1.0, behind);
        a.check("Shell · boxes, card (Dark, a palette)", &format!("titles (as rendered), over {bn}"), w, g, TEXT);
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, ink_bytes, list_dim, behind);
        a.check("Shell · boxes, card (Dark, a palette)", &format!("list rows (as rendered), over {bn}"), w, g, TEXT);
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, ink_srgb, list_dim, behind);
        a.check("Shell · boxes, card (Dark, a palette)", &format!("list rows (ink as sRGB), over {bn}"), w, g, TEXT);
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, s(shell.accent()), 1.0, behind);
        a.check("Shell · boxes, card (Dark, a palette)", &format!("accent words, over {bn}"), w, g, TEXT);
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, s(shell.c[8]), 1.0, behind);
        a.check("Shell · boxes, card (Dark, a palette)", &format!("red glyphs (×), over {bn}"), w, g, LARGE);
        // The dock: its ink measured on the fill as if solid; the glass a third, half open.
        let ink = if 0.2126 * fill[0] + 0.7152 * fill[1] + 0.0722 * fill[2] > 0.179 { [0.0; 3] } else { [1.0; 3] };
        // (Open: the resting dock carries no words; its names are tooltips, dense.)
        let al = DOCK_ALPHA;
        let (w, g) = layer(fill, al, ink, 1.0, behind);
        a.check("Shell · dock (Dark, a palette)", &format!("open box: names, over {bn}"), w, g, TEXT);
        let (w, g) = layer(fill, al, ink, crate::content::TITLE_ALPHA, behind);
        a.check("Shell · dock (Dark, a palette)", &format!("open box: section titles at .3, over {bn}"), w, g, LARGE);
        let (w, g) = layer(fill, al, ink, 0.45, behind);
        a.check("Shell · dock (Dark, a palette)", &format!("open box: placeholder, empty states at .45, over {bn}"), w, g, TEXT);
    }
    // Menus, Properties: solid.
    let (w, g) = layer(surface, 1.0, ink_bytes, 1.0, [0.0; 3]);
    a.check("Shell · menus (Dark, a palette)", "rows (as rendered)", w, g, TEXT);
    let (w, g) = layer(surface, 1.0, ink_bytes, 0.75, [0.0; 3]);
    a.check("Shell · menus (Dark, a palette)", "symbols at .75 (as rendered)", w, g, LARGE);
}

/// The Light look reads the screen: the same for every palette, so once,
/// over a sweep of greys and a few colours behind.
fn audit_screen_glass(a: &mut Audit) {
    a.pal = "(any)".into();
    a.design = true;
    let mut behind: Vec<(String, S)> = (0..=20).map(|i| (format!("grey {:.2}", i as f32 / 20.0), [i as f32 / 20.0; 3])).collect();
    for (n, c) in [("cream", [0.96, 0.94, 0.91]), ("sky", [0.55, 0.64, 0.74]), ("red", [0.8, 0.2, 0.2]), ("green", [0.2, 0.6, 0.3]), ("yellow", [0.95, 0.85, 0.3])] {
        behind.push((n.to_owned(), c));
    }
    for (bn, b) in &behind {
        a.tag = format!("Light look over {bn}");
        let bl = b.map(srgb_to_linear);
        // `Backdrop` with only a frost sample: not "bright" → the white wash.
        let fill = bl.map(|v| v * 0.89 + srgb_to_linear(0.11));
        let ink = if 0.2126 * fill[0] + 0.7152 * fill[1] + 0.0722 * fill[2] > 0.179 { [0.0; 3] } else { [1.0; 3] };
        let dim = if ink[0] < 0.5 { crate::options::LIST_DIM_LIGHT } else { crate::options::LIST_DIM };
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, ink, 1.0, *b);
        a.check("Shell · Light look (reads the screen)", "box titles", w, g, TEXT);
        let (w, g) = layer(fill, crate::options::BOX_ALPHA, ink, dim, *b);
        a.check("Shell · Light look (reads the screen)", "box list rows", w, g, TEXT);
        // The open dock: the toned glass over a near-white backdrop (`dock_surface`).
        let k = crate::options::tone_of([bl[0], bl[1], bl[2], 1.0]);
        let toned = crate::options::toned_fill([bl[0], bl[1], bl[2], 1.0]);
        let dfill: [f32; 3] = std::array::from_fn(|i| fill[i] + (toned[i] - fill[i]) * k);
        let al = DOCK_ALPHA + (toned[3] - DOCK_ALPHA) * k;
        let dink = if 0.2126 * dfill[0] + 0.7152 * dfill[1] + 0.0722 * dfill[2] > 0.179 { [0.0; 3] } else { [1.0; 3] };
        let (w, g) = layer(dfill, al, dink, 1.0, *b);
        a.check("Shell · Light look (reads the screen)", "open dock: names", w, g, TEXT);
        let (w, g) = layer(dfill, al, dink, crate::content::TITLE_ALPHA, *b);
        a.check("Shell · Light look (reads the screen)", "open dock: section titles at .3", w, g, LARGE);
        let (w, g) = layer(dfill, al, dink, 0.45, *b);
        a.check("Shell · Light look (reads the screen)", "open dock: placeholder, empty states at .45", w, g, TEXT);
    }
}

#[test]
#[ignore]
fn legibility_audit() {
    let mut a = Audit::default();
    let mut n = 0usize;
    let pals = all();
    for p in &pals {
        for dark in [true, false] {
            for light in [0u8, 25, 50, 75, 100] {
                for sat in [0u8, 50, 100] {
                    for tint in [Some(0u8), None, Some(100)] {
                        let choice = ColorChoice {
                            palette: Some(p.id.clone()),
                            shell_dark: Some(dark),
                            apps_dark: Some(dark),
                            web_dark: Some(dark),
                            shell_light: Some(light),
                            apps_light: Some(light),
                            web_light: Some(light),
                            shell_sat: Some(sat),
                            apps_sat: Some(sat),
                            web_sat: Some(sat),
                            shell_tint: tint,
                            apps_tint: tint,
                            web_tint: tint,
                            ..Default::default()
                        };
                        let shell = resolve(&choice, dark);
                        let apps = for_apps(&shell);
                        a.pal = p.id.clone();
                        a.design = light == 50 && sat == 50 && tint.is_none();
                        a.tag = format!("{} {} light={light} sat={sat} palette={}", p.id, if dark { "dark" } else { "light" }, tint.map_or("as made".to_owned(), |t| t.to_string()));
                        audit_apps(&mut a, &apps);
                        audit_web(&mut a, &apps, &shell, &choice);
                        audit_shell(&mut a, &choice, &shell, &apps, dark);
                        n += 1;
                    }
                }
            }
        }
        // Apps and shell in different looks (each row has its own Dark/Light).
        for (shell_dark, apps_dark) in [(false, true), (true, false)] {
            let choice = ColorChoice { palette: Some(p.id.clone()), shell_dark: Some(shell_dark), apps_dark: Some(apps_dark), ..Default::default() };
            let shell = resolve(&choice, shell_dark);
            let apps = for_apps(&shell);
            a.pal = p.id.clone();
            a.design = true;
            a.tag = format!("{} shell {} apps {}", p.id, if shell_dark { "dark" } else { "light" }, if apps_dark { "dark" } else { "light" });
            if !shell_dark {
                // Golem Files' menus in a Light shell: a fixed #e8e5de, the Apps' ink.
                let (w, g) = layer([0.807, 0.784, 0.73], 1.0, lin_of(apps.c[5]), 1.0, [0.0; 3]);
                a.check("Files", "menus in a Light shell, Apps Dark (as rendered)", w, g, TEXT);
            }
        }
    }
    audit_screen_glass(&mut a);
    // ---- the report
    let mut out = String::new();
    let _ = writeln!(out, "THE LEGIBILITY AUDIT: {} palettes, {n} cases, {} rules", pals.len(), a.rules.len());
    let mut group = String::new();
    let (mut rules_failing, mut checks, mut fails) = (0, 0, 0);
    for ((g, rule), st) in &a.rules {
        checks += st.cases;
        fails += st.fails;
        if st.fails == 0 {
            continue;
        }
        rules_failing += 1;
        if *g != group {
            let _ = writeln!(out, "\n## {g}");
            group.clone_from(g);
        }
        let _ = writeln!(
            out,
            "- {rule}: {} / {} fail ({:.0}%; as designed {}/{}), {} palettes, worst {:.2} ({})",
            st.fails,
            st.cases,
            100.0 * st.fails as f32 / st.cases as f32,
            st.design_fails,
            st.design_cases,
            st.pals.len(),
            st.worst.unwrap_or(0.0),
            st.ex
        );
    }
    let _ = writeln!(out, "\n{rules_failing} of {} rules fail somewhere; {fails} of {checks} checks.", a.rules.len());
    println!("{out}");
    if let Some(path) = std::env::var_os("AUDIT_OUT") {
        let _ = std::fs::write(path, &out);
    }
}

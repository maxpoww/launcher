//! THE CHROME of a file window — "A · Unified" (Max, 2026-10-21: "is
//! perfect! build it to the real golem files"; the mockup
//! https://claude.ai/artifact/DudbvDzykY1oVnMnmn7dvW). A standard file
//! manager's frame, not a new idea:
//!
//! - THE SIDEBAR, on the page's own colour with one hairline at its right:
//!   48 px of CARVED symbols at rest; the pointer on it opens it to 200 with
//!   the names, OVER the files, after a breath (`SIDE_WAIT`); leaving closes
//!   it. The PIN in its corner keeps it open beside the files
//!   (`settings.files_pinned`). At its top the two MODES, Files and Media:
//!   at rest only the one in use, open both as halves, the one in use sunken.
//! - ONE LINE along the top, fused with the files (no line under it): the
//!   TABS — the current one is the way here, each step clickable, ✎ and ×
//!   at its end on hover; the others fold to dimmed chips; + opens Home
//!   beside the current — and, at its right end, sort · i · grid/list ·
//!   search (a field slides out of it).
//! - THE i CARD under the line's right end: what is selected, or the folder.
//!
//! Golem's own title bar stays above it all with its back and forward
//! arrows: they act on the current tab (`filer_note_nav`).

use super::*;

/// The pointer rests this long on the sidebar before it opens.
pub(super) const SIDE_WAIT: Duration = Duration::from_millis(120);
/// The sidebar's air at its sides.
const SIDE_PAD: f32 = 8.0;
/// The two modes: their top, height and the gap between them open; the line
/// under them.
const MODES_TOP: f32 = 24.0;
const MODE_H: f32 = 28.0;
const MODE_GAP: f32 = 3.0;
const MODE_ICON: f32 = 14.0;
const MODES_LINE: f32 = 62.0;
/// Where the places begin, and the room a section's heading takes (12 over
/// its words, 4 under).
pub(super) const PLACES_TOP: f32 = MODES_LINE + 1.0;
pub(super) const SEC_H: f32 = 30.0;
/// The pin: its box in the corner, its symbol.
const PIN_BOX: f32 = 16.0;
const PIN_ICON: f32 = 11.0;
/// THE TOP LINE: its words, the air at its ends and between tabs.
const LINE_PX: f32 = 12.0;
const LINE_LH: f32 = 16.0;
const LINE_PAD: f32 = 12.0;
const TAB_GAP: f32 = 6.0;
const IN_GAP: f32 = 3.0;
/// The other tabs' chips; the current tab's steps, pencil and ×; the +.
const CHIP_H: f32 = 22.0;
const CHIP_MAX: f32 = 180.0;
const CHIP_ICON: f32 = 13.0;
const CRUMB_ICON: f32 = 14.0;
const CHEV: f32 = 8.0;
const PEN_BOX: f32 = 18.0;
/// KEYBOARD MODE's letters and what each does (`filer_key`): the pill's
/// glyph's words for when the shell's buttons are named (and the log's now).
pub(super) const KEYS: [(&str, &str); 26] = [
    ("W A S D", "Move around"),
    ("Shift", "Select a run"),
    ("Q", "Back"),
    ("E", "Open"),
    ("Z", "Undo"),
    ("X", "Cut"),
    ("C", "Copy"),
    ("V", "Paste"),
    ("R", "Rename"),
    ("Delete", "Move to bin"),
    ("U", "Up"),
    ("N", "New folder"),
    ("F", "Search"),
    ("G", "First"),
    ("Shift+G", "Last"),
    ("I", "Get info"),
    ("H", "Hidden files"),
    ("L", "Grid or list"),
    ("T", "New tab"),
    ("Shift+T", "Close tab"),
    ("/", "Golem Disk"),
    (".", "Home"),
    ("1–9", "A step of the path"),
    ("← →", "Previous or next tab"),
    ("?", "Keyboard shortcuts"),
    ("Ctrl+K", "Keyboard mode on/off"),
];

/// Keyboard mode's letters, said in the shell's language ("Q Back · …").
pub(super) fn keys_words() -> String {
    KEYS.iter()
        .map(|(k, what)| format!("{k} {}", tr(what)))
        .collect::<Vec<_>>()
        .join(" · ")
}
/// `..` with nowhere up: a disabled menu row's ink (`desktop_menu`'s 0.38).
const NOWHERE_INK: f32 = 0.35;
/// The path line's chip (`zones`): its height, air, words and lock.
const TAG_H: f32 = 18.0;
const TAG_PAD: f32 = 7.0;
const TAG_PX: f32 = 11.0;
const TAG_ICON: f32 = 10.0;
const PEN_ICON: f32 = 14.0;
const X_BOX: f32 = 16.0;
const X_ICON: f32 = 8.0;
const ADD_BOX: f32 = 22.0;
/// The field a path is typed in: never narrower than this.
const EDIT_MIN: f32 = 120.0;
/// The right end: its buttons, their symbols, the rules between them, the
/// search field.
const BTN_W: f32 = 22.0;
const BTN_H: f32 = 20.0;
const BTN_ICON: f32 = 12.0;
const SEEK_W: f32 = 170.0;
/// The ↶'s air before sort (it slides out of nothing to this).
const UNDO_GAP: f32 = 12.0;
/// THE HISTORY: its width (between), its most height, its air, its head,
/// a row, a row's Undo.
const HIST_MIN: f32 = 300.0;
const HIST_MAX: f32 = 340.0;
const HIST_H: f32 = 360.0;
const HIST_PAD: f32 = 6.0;
const HIST_HEAD: f32 = 24.0;
const HIST_ROW: f32 = 41.0;
const HIST_BTN_H: f32 = 21.0;
/// THE i CARD.
const CARD_W: f32 = 250.0;
const CARD_TOP: f32 = 40.0;
const CARD_PAD: f32 = 16.0;
const CARD_PIC: f32 = 130.0;
/// How fast a tab folds and unfolds, and the search field slides.
const TAB_RATE: f32 = 14.0;

/// One tab: its folder and its own way back and forward.
pub(super) struct Tab {
    pub(super) id: u64,
    pub(super) dir: PathBuf,
    pub(super) back: Vec<PathBuf>,
    pub(super) forward: Vec<PathBuf>,
}

impl Tab {
    pub(super) fn new(id: u64, dir: PathBuf) -> Self {
        Self {
            id,
            dir,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }
}

/// What the i card says: of the one thing selected (or the folder shown),
/// or of several.
pub(super) struct Info {
    /// What it was read for (`info_key`).
    pub(super) key: String,
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) rows: Vec<(String, String)>,
    /// Its picture: a key of the window's icons.
    pub(super) icon: String,
    /// What is free on the folder's disk.
    pub(super) free: Option<u64>,
}

// ── the chrome's own symbols ────────────────────────────────────────────

/// The symbols the chrome wears (the shell's own set: `glyphs.rs`).
const CHROME: [&str; 36] = [
    "clock", "pin", "files", "media", "sort", "info", "grid", "list", "eye", "eyeoff", "search",
    // (The system's chip on the path line; the confirmation's shield.)
    "lock", "shield", "keyboard", "pen", "x", "plus", "chev", "folder", "undo", "rename", "trash",
    "newf", "copy", "open", "dup", // (The selection pill's: there at once when it rises.)
    "move", "openwith", "usb", "more", "paste",
    // (And what a kind of thing is for: the pill's, and by the ↶.)
    "play", "slides", "pic", "print", "extract",
];

/// The icon key of symbol `name`.
pub(super) fn glyph(name: &str) -> String {
    crate::glyphs::key(name)
}

/// Every symbol the chrome wears, for the window's icon layers.
pub(super) fn glyph_keys() -> Vec<String> {
    CHROME.iter().map(|name| glyph(name)).collect()
}

/// Every icon the chrome wears now, for the window's layers: its symbols,
/// the folder of the steps and chips, a tab's place symbol, the i card's,
/// and those of the rows of a menu that is up.
pub(super) fn icon_keys(win: &Win) -> Vec<String> {
    let mut keys = glyph_keys();
    keys.push("asset:folder".to_owned());
    for p in win
        .places
        .iter()
        .filter(|p| p.path == home() || p.path == bin_root())
    {
        keys.push(symbolic(&p.icon));
    }
    if let Some(info) = win.info.as_ref().filter(|_| win.info_on) {
        keys.push(info.icon.clone());
    }
    if let Some(view) = win.pill_view.as_ref() {
        for part in &view.parts {
            if let PillPart::Button { icon, .. } = part {
                let key = glyph(icon);
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    if let Some(menu) = win.menu.as_ref() {
        for name in menu.glyphs() {
            let key = glyph(name);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        // (And the apps' own icons of its Another app page.)
        for key in menu.pictures() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// A place's icon as its symbol (the chrome's own are symbols already).
fn symbolic(icon: &str) -> String {
    if icon.ends_with(".svg") {
        icon.to_owned()
    } else {
        format!("{icon}-symbolic")
    }
}

// ── the tones ───────────────────────────────────────────────────────────

/// A linear colour's channel in sRGB, and back.
fn srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `a` `t` of the way to `b`, mixed as a page mixes them (in sRGB), solid.
pub(super) fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let m = |x: f32, y: f32| linear(srgb(x) + (srgb(y) - srgb(x)) * t);
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), 1.0]
}

/// The chrome's colours, all from the window's palette (the mockup's
/// tokens): over the ground, so they are solid.
pub(super) struct Tones {
    /// A place's symbol, the current place's, its sunken square.
    carved: [f32; 4],
    carved_lit: [f32; 4],
    sunk: [f32; 4],
    /// A symbol set back, and words (the ink, half through); the
    /// hairlines; a field's fill; a pointed-at button's; a chip pointed at.
    dim: [f32; 4],
    dim_text: [f32; 4],
    line: [f32; 4],
    field: [f32; 4],
    hover: [f32; 4],
    chip_on: [f32; 4],
    /// The i card: its fill and its rim. (Solid: the mockup's card is
    /// frosted glass over the files; a see-through card without the blur
    /// let the pictures under it read through its words.)
    card: [f32; 4],
    card_rim: [f32; 4],
}

pub(super) fn tones(pal: &Palette) -> Tones {
    let (g, ink) = (pal.ground, pal.ink);
    let (black, white) = ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
    if pal.dark {
        // DARK: the symbols follow the look like the words do — light grey,
        // the current one lighter on a lighter square (the mockup's #8a8a8d,
        // #c4c4c7 on #2e2e31 over #1e1e1e, as shares of the way to the ink).
        let card = mix(g, ink, 0.1);
        Tones {
            carved: mix(g, ink, 0.54),
            carved_lit: mix(g, ink, 0.83),
            sunk: mix(g, ink, 0.08),
            dim: mix(g, ink, 0.52),
            dim_text: [ink[0], ink[1], ink[2], 0.5],
            line: mix(g, white, 0.08),
            field: mix(g, white, 0.08),
            hover: mix(g, white, 0.06),
            chip_on: mix(g, white, 0.1),
            card,
            card_rim: mix(g, white, 0.12),
        }
    } else {
        // LIGHT: CARVED — the ground pressed in (black 42 % over it), the
        // current place a sunken square (16 %), its symbol deeper (55 %).
        let card = mix(g, ink, 0.03);
        Tones {
            carved: carve(g, 0.42),
            carved_lit: carve(g, 0.55),
            sunk: carve(g, 0.16),
            dim: mix(g, ink, 0.52),
            dim_text: [ink[0], ink[1], ink[2], 0.5],
            line: mix(g, black, 0.09),
            field: mix(g, black, 0.05),
            hover: mix(g, black, 0.05),
            chip_on: mix(g, black, 0.09),
            card,
            card_rim: mix(g, black, 0.1),
        }
    }
}

// ── drawing helpers ─────────────────────────────────────────────────────

fn measure(win: &mut Win, text: &str, px: f32, family: Option<&str>) -> f32 {
    win.renderer
        .as_mut()
        .map_or(text.chars().count() as f32 * px * 0.55, |r| {
            r.measure_text(text, px, family)
        })
}

/// Words of the chrome, in a colour of their own.
fn words(text: &str, (x, y): (f32, f32), max_w: f32, px: f32, color: [f32; 4]) -> Label {
    Label {
        font_px: px,
        line_px: px + 4.0,
        color: Some(color),
        ..label(text, (x, y), max_w, false, false)
    }
}

/// Leave out of `labels` every one drawn over `panel`, by the width it is
/// drawn at (`measure`). WORDS ARE PAINTED AFTER EVERY GRID (the renderer's
/// one text pass): one under a panel drawn later — a menu, the Properties
/// box, the history, the pill — showed through it while the symbols beside
/// it were covered (a 160 % menu over the selection pill: its words, and
/// not its symbols).
pub(super) fn leave_out(
    labels: &mut Vec<Label>,
    panel: Rect,
    measure: &mut dyn FnMut(&Label) -> f32,
) {
    labels.retain(|l| {
        let w = measure(l).min(l.max_w);
        let x = if l.centered {
            l.pos.0 - w / 2.0
        } else {
            l.pos.0
        };
        let h = l.line_px * (l.text.lines().count().max(1) as f32);
        !(x < panel.x + panel.w
            && panel.x < x + w
            && l.pos.1 < panel.y + panel.h
            && panel.y < l.pos.1 + h)
    });
}

/// Words of the top line, at its size.
fn words_at_line(text: &str, at: (f32, f32), max_w: f32, color: [f32; 4]) -> Label {
    words(text, at, max_w, LINE_PX, color)
}

/// A symbol in `color`, its alpha how much it shows (the icon washed whole
/// with the colour, then faded: `icon.wgsl`'s GHOST, `ring` -1.17).
fn symbol(grid: &mut GridContent, win: &Win, key: &str, r: Rect, color: [f32; 4]) {
    if let Some(layer) = win.layer_of(key) {
        grid.icons.push(IconInst {
            rect: r,
            layer,
            tint: [color[0], color[1], color[2], 1.0],
            ring: -1.17,
            plate: [0.0, 0.0, 0.0, color[3]],
        });
    }
}

/// An icon as it is (a folder's own colours), `alpha` of it.
fn picture(grid: &mut GridContent, win: &Win, key: &str, r: Rect, alpha: f32) {
    if let Some(layer) = win.layer_of(key) {
        let thumb = if key.starts_with("thumb:") {
            crate::content::PICTURE
        } else {
            0
        };
        let (ring, plate) = if alpha < 1.0 {
            (-1.17, [0.0, 0.0, 0.0, alpha])
        } else {
            (-1.0, NO_PLATE)
        };
        grid.icons.push(IconInst {
            rect: r,
            layer: layer | thumb,
            tint: [0.0; 4],
            ring,
            plate,
        });
    }
}

/// `r` centred on `c` (its middle), `s` square.
fn square(c: (f32, f32), s: f32) -> Rect {
    Rect::new((c.0 - s / 2.0).round(), (c.1 - s / 2.0).round(), s, s)
}

fn mid(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn faded(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The key the i card is read for: the selection, else the folder.
pub(super) fn info_key(win: &Win) -> String {
    if win.selected.is_empty() {
        return format!("dir:{}", win.dir.display());
    }
    let mut sel: Vec<&str> = win.selected.iter().map(String::as_str).collect();
    sel.sort_unstable();
    sel.join("\n")
}

/// What a tab is called: its folder's name, or the place's it is.
pub(super) fn tab_name(dir: &Path, places: &[Place]) -> String {
    places
        .iter()
        .find(|p| p.path == dir)
        .map_or_else(|| file_name(dir), |p| p.name.clone())
}

/// A step's own width: its folder (but the first's), its words, its air.
fn step_w(j: usize, text_w: f32) -> f32 {
    8.0 + text_w + if j > 0 { CRUMB_ICON + 4.0 } else { 0.0 }
}

/// The steps' width as drawn from `from` on (the chevrons between them in).
fn steps_w(widths: &[f32], from: usize) -> f32 {
    (from..widths.len())
        .map(|j| step_w(j, widths[j]) + if j > from { CHEV + 2.0 * IN_GAP } else { 0.0 })
        .sum()
}

impl App {
    /// Draw window `win`'s chrome (its sidebar, its top line, the i card) and
    /// note where its parts are in `hits`. The grids are drawn after the
    /// files' in this order; `moving` asks for another frame.
    pub(super) fn filer_chrome(
        &mut self,
        win: &mut Win,
        pal: &Palette,
        hits: &mut Hits,
        dt: f32,
    ) -> (Vec<GridContent>, bool) {
        let tones = tones(pal);
        let mut moving = false;
        // The search field slides out and in.
        let (t, m) = crate::animation::ease_toward(
            win.find_t,
            if win.find.is_some() { 1.0 } else { 0.0 },
            dt,
            TAB_RATE,
            0.004,
        );
        win.find_t = t;
        moving |= m;
        // The ↶ slides out while something can be taken back, and away.
        let (t, m) = crate::animation::ease_toward(
            win.undo_t,
            if win.undoable() { 1.0 } else { 0.0 },
            dt,
            TAB_RATE,
            0.004,
        );
        win.undo_t = t;
        moving |= m;
        let side_w = win.side_now();
        let head = self.filer_head(win, pal, &tones, hits, side_w, dt, &mut moving);
        let card = self.filer_card(win, pal, &tones, hits);
        let mut grids = vec![head];
        grids.extend(card);
        if win.has_side() {
            grids.extend(self.filer_sidebar(win, pal, &tones, hits, side_w));
        } else {
            hits.side = Rect::default();
            hits.mode = [Rect::default(); 2];
        }
        // The selection pill over the files, and the history over all of
        // it (the words under either left out: words are painted last).
        let mut covers = Vec::new();
        if let Some(pill) = self.filer_pill(win, pal, hits, dt, &mut moving) {
            covers.push((pill, hits.pill));
        }
        if let Some(hist) = self.filer_history(win, pal, hits) {
            covers.push((hist, hits.hist));
        }
        for (cover, panel) in covers {
            let under = |l: &Label| {
                let r = Rect::new(l.pos.0, l.pos.1, l.max_w, l.line_px);
                r.x < panel.x + panel.w
                    && panel.x < r.x + r.w
                    && r.y < panel.y + panel.h
                    && panel.y < r.y + r.h
            };
            for g in &mut grids {
                g.labels.retain(|l| !under(l));
            }
            grids.push(cover);
        }
        // (A word's own clip stands in for its grid's: keep both — the open
        // sidebar covers the top line's left, and the words are painted last.)
        for g in &mut grids {
            for l in &mut g.labels {
                l.clip = Some(l.clip.map_or(g.clip, |c| c.intersect(&g.clip)));
            }
        }
        (grids, moving)
    }

    /// THE SIDEBAR: its ground and hairline, the pin, the two modes, the
    /// places in their sections.
    fn filer_sidebar(
        &mut self,
        win: &mut Win,
        pal: &Palette,
        tones: &Tones,
        hits: &mut Hits,
        side_w: f32,
    ) -> [GridContent; 2] {
        let (_, h) = win.virt();
        let ink = pal.ink;
        let open = if win.pinned {
            1.0
        } else {
            smooth((win.side_t - 0.3) / 0.7)
        };
        let mut top = GridContent {
            clip: Rect::new(0.0, 0.0, side_w, h),
            ..Default::default()
        };
        hits.side = top.clip;
        top.rects.push(RectInst {
            glass: crate::content::UNLIT,
            ..rect(top.clip, 0.0, pal.ground)
        });
        top.rects
            .push(rect(Rect::new(side_w - 1.0, 0.0, 1.0, h), 0.0, tones.line));
        let inner = side_w - 2.0 * SIDE_PAD;

        // The pin: in the corner, only while the sidebar is open; lit (white)
        // while pinned.
        hits.pin = Rect::default();
        if open > 0.0 {
            let r = Rect::new(side_w - 6.0 - PIN_BOX, 4.0, PIN_BOX, PIN_BOX);
            hits.pin = r;
            let at = square(mid(r), PIN_ICON);
            if win.pinned {
                if !pal.dark {
                    // (A hairline dark edge keeps the white seen on white.)
                    for (dx, dy) in [(-0.6, 0.0), (0.6, 0.0), (0.0, -0.6), (0.0, 0.8)] {
                        let r = Rect::new(at.x + dx, at.y + dy, at.w, at.h);
                        symbol(&mut top, win, &glyph("pin"), r, [0.0, 0.0, 0.0, 0.5 * open]);
                    }
                }
                symbol(&mut top, win, &glyph("pin"), at, [1.0, 1.0, 1.0, open]);
            } else {
                let c = if win.hover == Spot::Pin {
                    tones.carved_lit
                } else {
                    tones.carved
                };
                symbol(&mut top, win, &glyph("pin"), at, faded(c, open));
            }
        }

        // The modes: at rest only the one in use; open, the two as halves,
        // the one in use sunken.
        let half = (inner - MODE_GAP) / 2.0;
        let lit = usize::from(win.media);
        let width = |n: usize| {
            if n == lit {
                inner + (half - inner) * open
            } else {
                half * open
            }
        };
        let w0 = width(0);
        let boxes = [
            Rect::new(SIDE_PAD, MODES_TOP, w0, MODE_H),
            Rect::new(SIDE_PAD + w0 + MODE_GAP * open, MODES_TOP, width(1), MODE_H),
        ];
        for (n, (b, (sign, name))) in boxes
            .iter()
            .zip([("files", tr("Files")), ("media", tr("Media"))])
            .enumerate()
        {
            if b.w < 1.0 {
                continue;
            }
            let alpha = if n == lit { 1.0 } else { open };
            if n == lit {
                top.rects.push(rect(*b, MODE_H / 2.0, tones.sunk));
            } else if win.hover == Spot::Mode(n) {
                top.rects
                    .push(rect(*b, MODE_H / 2.0, faded(tones.hover, alpha)));
            }
            let name_w = measure(win, name, 13.0, None);
            // (The symbol, and its name beside it once there is room.)
            let full = MODE_ICON + 6.0 + name_w;
            let shown = (b.w - 12.0).min(full);
            let x0 = b.x + (b.w - shown.max(MODE_ICON)) / 2.0;
            let c = if n == lit {
                tones.carved_lit
            } else {
                tones.carved
            };
            symbol(
                &mut top,
                win,
                &glyph(sign),
                square((x0 + MODE_ICON / 2.0, mid(*b).1), MODE_ICON),
                faded(c, alpha),
            );
            if open > 0.0 {
                top.labels.push(Label {
                    clip: Some(*b),
                    ..words(
                        name,
                        (x0 + MODE_ICON + 6.0, mid(*b).1 - 8.5),
                        name_w + 2.0,
                        13.0,
                        faded(ink, open * alpha),
                    )
                });
            }
        }
        hits.mode = boxes;
        top.rects.push(rect(
            Rect::new(SIDE_PAD, MODES_LINE, inner, 1.0),
            0.0,
            tones.line,
        ));

        // The places, in their sections; scrolled when they are more than
        // the window shows.
        let mut list = GridContent {
            clip: Rect::new(0.0, PLACES_TOP, side_w - 1.0, (h - PLACES_TOP).max(0.0)),
            ..Default::default()
        };
        let (icon, row_h) = (win.place_icon(), win.place_row());
        let mut y = PLACES_TOP - win.side_scroll;
        let heading = |list: &mut GridContent, y: f32, text: &str| {
            list.labels.push(Label {
                family: Some(crate::content::FONT_BOLD),
                ..words(
                    text,
                    (SIDE_PAD * 2.0, y + 12.0),
                    inner,
                    11.0,
                    faded(tones.dim_text, open),
                )
            });
        };
        hits.places.clear();
        // (`..` stands first, over Favourites' heading.)
        let mut headed = false;
        for n in 0..win.places.len() {
            let place = &win.places[n];
            if !headed && place.name != UP {
                heading(&mut list, y, tr("Favourites"));
                y += SEC_H;
                headed = true;
            }
            if place.gap {
                heading(&mut list, y, tr("Locations"));
                y += SEC_H;
            }
            let row = Rect::new(SIDE_PAD, y, inner, row_h);
            y += row_h;
            if row.y + row.h <= PLACES_TOP || row.y >= h {
                hits.places.push(Rect::default());
                continue;
            }
            // (A place a drag is held on wears the box — and loses it for
            // a blink just before it opens.)
            let here = (win.dir == place.path && place.name != UP)
                || (win.drop_place == Some(n) && win.place_blink != Some(n));
            // (`..` with nowhere up: in its spot, at a disabled row's ink.)
            let dim = if place.nowhere() { NOWHERE_INK } else { 1.0 };
            if here {
                list.rects.push(rect(row, 6.0, tones.sunk));
            } else if win.hover == Spot::Place(n) {
                list.rects.push(rect(row, 6.0, tones.hover));
            }
            let key = win.place_key(place);
            let at = Rect::new(row.x + 8.0, row.y + (row_h - icon) / 2.0, icon, icon);
            if key.ends_with("-symbolic") || key.ends_with(".svg") {
                symbol(
                    &mut list,
                    win,
                    &key,
                    at,
                    faded(if here { tones.carved_lit } else { tones.carved }, dim),
                );
            } else {
                picture(&mut list, win, &key, at, dim);
            }
            if open > 0.0 {
                let name = place.name.clone();
                list.labels.push(Label {
                    clip: Some(row),
                    ..words(
                        &name,
                        (row.x + 8.0 + icon + 9.0, row.y + (row_h - 17.0) / 2.0),
                        inner - icon - 25.0,
                        13.0,
                        faded(ink, open * dim),
                    )
                });
            }
            hits.places.push(Rect::new(
                row.x,
                row.y.max(PLACES_TOP),
                row.w,
                row.y + row.h - row.y.max(PLACES_TOP),
            ));
        }
        [top, list]
    }

    /// THE TOP LINE: the tabs and the way here, then sort · i · grid/list ·
    /// the eye · search at its right end.
    #[allow(clippy::too_many_arguments)]
    fn filer_head(
        &mut self,
        win: &mut Win,
        pal: &Palette,
        tones: &Tones,
        hits: &mut Hits,
        side_w: f32,
        dt: f32,
        moving: &mut bool,
    ) -> GridContent {
        let (w, _) = win.virt();
        let ink = pal.ink;
        let left = win.field_left();
        let mut g = GridContent {
            clip: Rect::new(
                side_w.max(left),
                0.0,
                (w - side_w.max(left)).max(0.0),
                HEAD_H,
            ),
            ..Default::default()
        };
        g.rects.push(rect(
            Rect::new(left, 0.0, w - left, HEAD_H),
            0.0,
            pal.ground,
        ));
        let cy = HEAD_H / 2.0;
        let text_y = cy - LINE_LH / 2.0;

        // ── the right end ──
        let e = smooth(win.find_t);
        let find = Rect::new(w - 10.0 - BTN_W, cy - BTN_H / 2.0, BTN_W, BTN_H);
        let seek = Rect::new(find.x - 6.0 * e - SEEK_W * e, cy - 9.0, SEEK_W * e, 18.0);
        // SHOW HIDDEN FILES (Max, 2026-10-22: "agrega a los botones 'see
        // hidden files' a la izquierda de buscar"): left of where the search
        // field slides out, its symbol the state — the slashed eye while
        // they are not shown, the eye, pressed, while they are ("el icono
        // debería cambiar").
        let hidden = Rect::new(
            seek.x - 4.0 * e - 1.0 - BTN_W,
            cy - BTN_H / 2.0,
            BTN_W,
            BTN_H,
        );
        let more = hidden.x;
        let at = |dx: f32| Rect::new(more - 107.0 + dx, cy - BTN_H / 2.0, BTN_W, BTN_H);
        let (sort, info, grid, list) = (at(0.0), at(23.0), at(54.0), at(77.0));
        let rules = [more - 107.0 + 49.0, more - 107.0 + 103.0];
        // The ↶ left of sort: out of nothing (its width and its air) while
        // something can be taken back.
        // The folder's own action, when it is mostly songs or photos ("Play
        // all", "Slideshow"): standing still left of sort, the ↶ sliding in
        // left of it (so it does not move when the ↶ comes).
        let here = match Self::filer_here_kind(win) {
            Some(super::smart::FolderKind::Play) => {
                Some(("play", tr("Play all"), Action::PlayHere))
            }
            Some(super::smart::FolderKind::Slides) => {
                Some(("slides", tr("Slideshow"), Action::SlideshowHere))
            }
            None => None,
        };
        let here_rect = here.map(|(_, words, _)| {
            let w = 7.0 + BTN_ICON + 5.0 + measure(win, words, LINE_PX, None) + 8.0;
            Rect::new(sort.x - UNDO_GAP - w, cy - BTN_H / 2.0, w, BTN_H)
        });
        let u = smooth(win.undo_t);
        let undo = Rect::new(
            here_rect.map_or(sort.x, |r| r.x) - (BTN_W + UNDO_GAP) * u,
            cy - BTN_H / 2.0,
            BTN_W * u,
            BTN_H,
        );
        let tools_left = undo.x - 6.0;
        for x in rules {
            g.rects
                .push(rect(Rect::new(x, cy - 5.0, 1.0, 10.0), 0.0, tones.line));
        }
        let button =
            |g: &mut GridContent, win: &Win, r: Rect, sign: &str, pressed: bool, spot: Spot| {
                let pointed = win.hover == spot;
                if pressed {
                    g.rects.push(rect(r, 6.0, tones.field));
                } else if pointed {
                    g.rects.push(rect(r, 6.0, tones.hover));
                }
                let c = if pressed || pointed { ink } else { tones.dim };
                symbol(g, win, &glyph(sign), square(mid(r), BTN_ICON), c);
            };
        button(
            &mut g,
            win,
            sort,
            "sort",
            win.menu
                .as_ref()
                .is_some_and(|m| m.item.is_none() && win.menu_path.as_deref() == Some(SORT_MENU)),
            Spot::Sort,
        );
        button(&mut g, win, info, "info", win.info_on, Spot::Info);
        hits.here = None;
        if let (Some((sign, words, action)), Some(r)) = (here, here_rect) {
            let pointed = win.hover == Spot::Here;
            if pointed {
                g.rects.push(rect(r, 6.0, tones.hover));
            }
            let c = if pointed { ink } else { tones.dim };
            symbol(
                &mut g,
                win,
                &glyph(sign),
                square((r.x + 7.0 + BTN_ICON / 2.0, cy), BTN_ICON),
                c,
            );
            g.labels.push(words_at_line(
                words,
                (r.x + 7.0 + BTN_ICON + 5.0, text_y),
                r.w,
                if pointed { ink } else { tones.dim_text },
            ));
            hits.here = Some((r, action));
        }
        hits.undo = Rect::default();
        if u > 0.01 {
            // (Its symbol fades with it: it is not a dimmed button, it is
            // there or it is not.)
            let pointed = win.hover == Spot::Undo;
            if win.hist_open {
                g.rects.push(rect(undo, 6.0, faded(tones.field, u)));
            } else if pointed {
                g.rects.push(rect(undo, 6.0, faded(tones.hover, u)));
            }
            let c = if win.hist_open || pointed {
                ink
            } else {
                tones.dim
            };
            symbol(
                &mut g,
                win,
                &glyph("undo"),
                square(mid(undo), BTN_ICON),
                faded(c, u * u),
            );
            if win.undoable() {
                hits.undo = undo;
            }
        }
        button(&mut g, win, grid, "grid", !win.list, Spot::View(false));
        button(&mut g, win, list, "list", win.list, Spot::View(true));
        let eye = if win.hidden { "eye" } else { "eyeoff" };
        button(&mut g, win, hidden, eye, win.hidden, Spot::Hidden);
        // While the search is out, its button is its ×: a click cancels it
        // as Escape in the field does (Max, 2026-10-22) — "Search" /
        // "Cancel search".
        let sign = if win.find.is_some() { "x" } else { "search" };
        button(&mut g, win, find, sign, win.find.is_some(), Spot::Find);
        (
            hits.sort,
            hits.info,
            hits.grid,
            hits.list,
            hits.hidden,
            hits.find,
            hits.seek,
        ) = (sort, info, grid, list, hidden, find, seek);
        if seek.w > 1.0 {
            // The search: what is typed, or what it searches.
            let typed = win.find.clone().unwrap_or_default();
            if typed.is_empty() {
                let folder = tab_name(&win.dir, &win.places);
                let ask = fill(tr("Search {folder}"), &[("folder", &folder)]);
                g.labels.push(Label {
                    clip: Some(seek),
                    ..words(
                        &ask,
                        (seek.x, text_y),
                        SEEK_W,
                        LINE_PX,
                        faded(tones.dim_text, e),
                    )
                });
            } else {
                g.labels.push(Label {
                    clip: Some(seek),
                    cache: false,
                    ..words(
                        &typed,
                        (seek.x, text_y),
                        SEEK_W + 400.0,
                        LINE_PX,
                        faded(ink, e),
                    )
                });
            }
            if win.find.is_some() {
                let tw = measure(win, &typed, LINE_PX, None).min(seek.w - 2.0);
                g.rects.push(rect(
                    Rect::new(seek.x + tw + 1.0, text_y + 1.0, 1.0, LINE_LH - 2.0),
                    0.0,
                    ink,
                ));
            }
        }

        // ── the tabs ──
        let many = win.tabs.len() > 1;
        let x0 = left + LINE_PAD;
        let mut cursor = x0;
        // (The others' chips first measured: the current tab gets what is left.)
        let chip_words: Vec<(String, f32)> = (0..win.tabs.len())
            .map(|i| {
                let name = if i == win.tab {
                    String::new()
                } else {
                    tab_name(&win.tabs[i].dir, &win.places)
                };
                let tw = measure(win, &name, LINE_PX, None);
                (name, tw)
            })
            .collect();
        // (A few pixels to spare: the name is fitted with a little slack.)
        let chip_w = |tw: f32| (8.0 + CHIP_ICON + 5.0 + tw + 6.0 + 5.0 + X_BOX + 3.0).min(CHIP_MAX);
        let others: f32 = (0..win.tabs.len())
            .filter(|&i| i != win.tab)
            .map(|i| chip_w(chip_words[i].1) + TAB_GAP)
            .sum();
        // THE PATH LINE'S CHIP (`zones`, Max 2026-10-22): at the end of the
        // way here, small and dim — a lock and "System" where nothing is
        // the owner's to change, "Emptied at restart" in /tmp and /var/tmp.
        let editing = win.go_to.is_some();
        let tag: Option<(bool, &str)> = if editing || win.dir == super::recent::root() {
            None
        } else if win.zone == super::zones::Zone::System {
            Some((true, tr("System")))
        } else if super::zones::emptied_at_restart(&win.dir) {
            Some((false, tr("Emptied at restart")))
        } else {
            None
        };
        let tag_w = tag.map_or(0.0, |(lock, words)| {
            let lock_w = if lock { TAG_ICON + 4.0 } else { 0.0 };
            IN_GAP + 2.0 * TAG_PAD + lock_w + measure(win, words, TAG_PX, None)
        });
        let tail = IN_GAP + PEN_BOX + if many { IN_GAP + X_BOX } else { 0.0 } + tag_w;
        let room = (tools_left - x0 - others - TAB_GAP - ADD_BOX - tail).max(40.0);
        let steps = crumbs(&win.dir, &win.places);
        let widths: Vec<f32> = steps
            .iter()
            .map(|(n, _)| measure(win, n, LINE_PX, None))
            .collect();
        // (The first steps give way when the line is short.)
        let mut from = steps.len().saturating_sub(1);
        while from > 0 && steps_w(&widths, from - 1) <= room {
            from -= 1;
        }
        let field_w = (win.edit_w - tail).max(EDIT_MIN);
        let current_w = if editing {
            field_w
        } else {
            steps_w(&widths, from).min(room)
        } + tail;
        hits.tabs.clear();
        hits.tab_x.clear();
        hits.crumbs.clear();
        hits.pencil = Rect::default();
        hits.cur_tab = Rect::default();
        let on_current = win
            .ptr
            .is_some_and(|p| p.1 < HEAD_H && win.hits.cur_tab.contains(p));
        for (i, (chip_name, chip_tw)) in chip_words.iter().enumerate() {
            let id = win.tabs[i].id;
            let target = if i == win.tab {
                current_w
            } else {
                chip_w(*chip_tw)
            };
            let drawn = win.tab_w.entry(id).or_insert(0.0);
            let (now, m) = crate::animation::ease_toward(*drawn, target, dt, TAB_RATE, 0.3);
            *drawn = now;
            *moving |= m;
            let tw = now;
            let clip = Rect::new(cursor, 0.0, tw, HEAD_H);
            if i == win.tab {
                hits.cur_tab = clip;
                let mut x = cursor;
                if editing {
                    // The way here as a field: the whole path, its end in sight.
                    let text = win.go_to.clone().unwrap_or_default();
                    let field = Rect::new(x, cy - CHIP_H / 2.0, field_w.min(tw), CHIP_H);
                    g.rects.push(rect(field, 6.0, tones.field));
                    let text_w = measure(win, &text, LINE_PX, None);
                    let tx = (field.x + 9.0).min(field.x + field.w - 9.0 - text_w);
                    let color = if win.go_bad {
                        [0.79, 0.06, 0.07, 1.0]
                    } else {
                        ink
                    };
                    g.labels.push(Label {
                        clip: Some(Rect::new(field.x + 4.0, field.y, field.w - 8.0, field.h)),
                        cache: false,
                        ..words(&text, (tx, text_y), text_w + 4.0, LINE_PX, color)
                    });
                    g.rects.push(rect(
                        Rect::new(tx + text_w + 1.0, text_y + 1.0, 1.0, LINE_LH - 2.0),
                        0.0,
                        ink,
                    ));
                    x += field_w;
                } else {
                    for j in from..steps.len() {
                        let (name, dir) = steps[j].clone();
                        if j > from {
                            x += IN_GAP;
                            symbol(
                                &mut g,
                                win,
                                &glyph("chev"),
                                square((x + CHEV / 2.0, cy), CHEV),
                                tones.dim,
                            );
                            x += CHEV + IN_GAP;
                        }
                        let bw = step_w(j, widths[j])
                            .min(clip.x + clip.w - tail - x)
                            .max(0.0);
                        let hit = Rect::new(x, cy - 10.0, bw, 20.0);
                        if win.hover == Spot::Crumb(hits.crumbs.len()) {
                            g.rects.push(rect(hit, 4.0, tones.hover));
                        }
                        let mut tx = x + 4.0;
                        if j > 0 {
                            picture(
                                &mut g,
                                win,
                                "asset:folder",
                                square((tx + CRUMB_ICON / 2.0, cy), CRUMB_ICON),
                                1.0,
                            );
                            tx += CRUMB_ICON + 4.0;
                        }
                        g.labels.push(Label {
                            clip: Some(hit),
                            ..words(&name, (tx, text_y), widths[j] + 2.0, LINE_PX, ink)
                        });
                        hits.crumbs.push((hit, dir));
                        x += bw;
                    }
                    if let Some((lock, words_now)) = tag {
                        let chip = Rect::new(x + IN_GAP, cy - TAG_H / 2.0, tag_w - IN_GAP, TAG_H);
                        // (Its words at the menus' ink, three quarters: read at
                        // a glance, not shouting; its fill a step stronger
                        // than a field's.)
                        let tag_ink = faded(ink, 0.75);
                        g.rects.push(rect(chip, TAG_H / 2.0, tones.chip_on));
                        let mut tx = chip.x + TAG_PAD;
                        if lock {
                            symbol(
                                &mut g,
                                win,
                                &glyph("lock"),
                                square((tx + TAG_ICON / 2.0, cy), TAG_ICON),
                                mix(tones.chip_on, ink, 0.75),
                            );
                            tx += TAG_ICON + 4.0;
                        }
                        g.labels.push(Label {
                            clip: Some(chip),
                            ..words(
                                words_now,
                                (tx, cy - (TAG_PX + 4.0) / 2.0),
                                chip.w,
                                TAG_PX,
                                tag_ink,
                            )
                        });
                        x += tag_w;
                    }
                }
                // ✎ and ×: on hover (the pencil also while it is taken).
                let pen = Rect::new(x + IN_GAP, cy - PEN_BOX / 2.0, PEN_BOX, PEN_BOX);
                hits.pencil = pen;
                if on_current || editing {
                    if win.hover == Spot::Pencil {
                        g.rects.push(rect(pen, 4.0, tones.hover));
                    }
                    let c = if editing || win.hover == Spot::Pencil {
                        ink
                    } else {
                        tones.dim
                    };
                    symbol(&mut g, win, &glyph("pen"), square(mid(pen), PEN_ICON), c);
                }
                if many {
                    let xr = Rect::new(pen.x + PEN_BOX + IN_GAP, cy - X_BOX / 2.0, X_BOX, X_BOX);
                    hits.tab_x.push((xr, i));
                    if on_current || win.hover == Spot::TabX(i) {
                        if win.hover == Spot::TabX(i) {
                            g.rects.push(rect(xr, 4.0, tones.hover));
                        }
                        let c = if win.hover == Spot::TabX(i) {
                            ink
                        } else {
                            tones.dim
                        };
                        symbol(&mut g, win, &glyph("x"), square(mid(xr), X_ICON), c);
                    }
                }
            } else {
                // Another tab: a chip, set back until it is pointed at.
                let pointed = matches!(win.hover, Spot::Tab(n) | Spot::TabX(n) if n == i);
                let chip = Rect::new(cursor, cy - CHIP_H / 2.0, tw, CHIP_H);
                let a = if pointed { 1.0 } else { 0.62 };
                g.rects.push(rect(
                    chip,
                    6.0,
                    faded(
                        if pointed { tones.chip_on } else { tones.field },
                        if pointed { 1.0 } else { 0.62 },
                    ),
                ));
                let dir = win.tabs[i].dir.clone();
                let sign = win
                    .places
                    .iter()
                    .find(|p| p.path == dir)
                    .map(|p| p.icon.clone());
                let ic = square((chip.x + 8.0 + CHIP_ICON / 2.0, cy), CHIP_ICON);
                if ic.x + ic.w <= chip.x + chip.w {
                    match sign.filter(|_| {
                        dir == home() || dir == bin_root() || dir == super::recent::root()
                    }) {
                        Some(sign) => {
                            symbol(&mut g, win, &symbolic(&sign), ic, faded(pal.accent, a))
                        }
                        None => picture(&mut g, win, "asset:folder", ic, a),
                    }
                }
                let (name, nw) = (chip_name.clone(), *chip_tw);
                let room = (chip.w - (8.0 + CHIP_ICON + 5.0) - (5.0 + X_BOX + 3.0)).max(0.0);
                let shown =
                    crate::desktop::fit_label(&name, room, &mut |t| measure(win, t, LINE_PX, None));
                g.labels.push(Label {
                    clip: Some(chip),
                    ..words(
                        &shown,
                        (chip.x + 8.0 + CHIP_ICON + 5.0, text_y),
                        nw.min(room) + 2.0,
                        LINE_PX,
                        faded(if pointed { ink } else { tones.dim_text }, a),
                    )
                });
                let xr = Rect::new(
                    chip.x + chip.w - 3.0 - X_BOX,
                    cy - X_BOX / 2.0,
                    X_BOX,
                    X_BOX,
                );
                if pointed {
                    if win.hover == Spot::TabX(i) {
                        g.rects.push(rect(xr, 4.0, tones.hover));
                    }
                    symbol(
                        &mut g,
                        win,
                        &glyph("x"),
                        square(mid(xr), X_ICON),
                        if win.hover == Spot::TabX(i) {
                            ink
                        } else {
                            tones.dim
                        },
                    );
                }
                hits.tab_x.push((xr, i));
                hits.tabs.push((chip, i));
            }
            cursor += tw + TAB_GAP;
        }
        // The +: a Home tab right of the current one.
        let add = Rect::new(cursor, cy - ADD_BOX / 2.0, ADD_BOX, ADD_BOX);
        hits.tab_add = add;
        if win.hover == Spot::TabAdd {
            g.rects.push(rect(add, 6.0, tones.hover));
        }
        symbol(
            &mut g,
            win,
            &glyph("plus"),
            square(mid(add), BTN_ICON),
            if win.hover == Spot::TabAdd {
                ink
            } else {
                tones.dim
            },
        );
        // (Tabs that are gone forget their width.)
        let ids: HashSet<u64> = win.tabs.iter().map(|t| t.id).collect();
        win.tab_w.retain(|id, _| ids.contains(id));
        g
    }

    /// THE i CARD: what is selected — one thing, several — or the folder.
    fn filer_card(
        &mut self,
        win: &mut Win,
        pal: &Palette,
        tones: &Tones,
        hits: &mut Hits,
    ) -> Option<GridContent> {
        hits.card = Rect::default();
        if !win.info_on {
            return None;
        }
        let (w, h) = win.virt();
        let ink = pal.ink;
        let x = w - 12.0 - CARD_W;
        let inner = CARD_W - 2.0 * CARD_PAD;
        let mut g = GridContent {
            clip: Rect::new(x - 24.0, CARD_TOP - 12.0, CARD_W + 48.0, h - CARD_TOP),
            ..Default::default()
        };
        let key = info_key(win);
        // (What it says, held apart from the window: the words are measured.)
        let info = win.info.as_ref().filter(|i| i.key == key).map(|i| {
            (
                i.name.clone(),
                i.kind.clone(),
                i.rows.clone(),
                i.icon.clone(),
                i.free,
            )
        });
        // Its lines laid out first: the card is as tall as they are.
        let mut y = CARD_TOP + CARD_PAD;
        let pic = Rect::new(x + (CARD_W - CARD_PIC) / 2.0, y, CARD_PIC, CARD_PIC);
        y += CARD_PIC + 12.0;
        let name_y = y;
        y += 18.0;
        let kind_y = y;
        y += 16.0 + 12.0;
        let rows: Vec<(String, String)> = info.as_ref().map(|i| i.2.clone()).unwrap_or_default();
        let rule1 = y;
        if !rows.is_empty() {
            y += 12.0 + rows.len() as f32 * 22.0 - 6.0;
        }
        let rule2 = y + 12.0;
        y = rule2 + 10.0;
        let about_y = y;
        y += 16.0 + CARD_PAD;
        let card = Rect::new(x, CARD_TOP, CARD_W, (y - CARD_TOP).min(h - CARD_TOP - 16.0));
        hits.card = card;
        // A soft shadow, the card, its hairline.
        for k in 1..=5 {
            let k = k as f32;
            let grow = 2.0 * k;
            let s = Rect::new(
                card.x - grow,
                card.y - grow + 6.0,
                card.w + 2.0 * grow,
                card.h + 2.0 * grow,
            );
            g.rects.push(rect(
                s,
                12.0 + grow,
                [0.0, 0.0, 0.0, 0.035 * (1.0 - k / 6.0)],
            ));
        }
        g.rects.push(RectInst {
            glass: crate::content::UNLIT,
            ..rect(card, 12.0, tones.card)
        });
        g.rects.push(RectInst {
            border: 1.0,
            ..rect(card, 12.0, faded(tones.card_rim, 0.6))
        });
        let Some((info_name, info_kind, _, icon, free)) = info else {
            return Some(g);
        };
        let size = if icon.starts_with("thumb:") {
            CARD_PIC
        } else {
            96.0
        };
        if icon.ends_with(".svg") {
            // (A symbol of the chrome's own — Recents' clock.)
            symbol(&mut g, win, &icon, square(mid(pic), 72.0), tones.dim);
        } else {
            picture(&mut g, win, &icon, square(mid(pic), size), 1.0);
        }
        let fit = |win: &mut Win, text: &str, room: f32, px: f32, family: Option<&str>| {
            crate::desktop::fit_label(text, room, &mut |t| measure(win, t, px, family))
        };
        let name = fit(
            win,
            &info_name,
            inner,
            13.0,
            Some(crate::content::FONT_BOLD),
        );
        g.labels.push(Label {
            family: Some(crate::content::FONT_BOLD),
            centered: true,
            ..words(&name, (x + CARD_W / 2.0, name_y), inner, 13.0, ink)
        });
        g.labels.push(Label {
            centered: true,
            ..words(
                &info_kind,
                (x + CARD_W / 2.0, kind_y),
                inner,
                LINE_PX,
                tones.dim_text,
            )
        });
        if !rows.is_empty() {
            g.rects.push(rect(
                Rect::new(x + CARD_PAD, rule1, inner, 1.0),
                0.0,
                tones.card_rim,
            ));
            let label_w = rows
                .iter()
                .map(|(k, _)| measure(win, k, LINE_PX, None))
                .fold(0.0, f32::max)
                .min(inner * 0.45);
            let mut ry = rule1 + 12.0;
            for (k, v) in &rows {
                let kw = measure(win, k, LINE_PX, None).min(label_w);
                g.labels.push(words(
                    k,
                    (x + CARD_PAD + label_w - kw, ry),
                    label_w + 2.0,
                    LINE_PX,
                    tones.dim_text,
                ));
                let room = inner - label_w - 12.0;
                let value = fit(win, v, room, LINE_PX, None);
                g.labels.push(words(
                    &value,
                    (x + CARD_PAD + label_w + 12.0, ry),
                    room + 2.0,
                    LINE_PX,
                    ink,
                ));
                ry += 22.0;
            }
        }
        g.rects.push(rect(
            Rect::new(x + CARD_PAD, rule2, inner, 1.0),
            0.0,
            tones.card_rim,
        ));
        // The foot: how many, how many selected, what is free.
        let n = win.items.len();
        let picked = win.selected.len();
        let free = free
            .map(crate::desktop_props::size_text)
            .unwrap_or_default();
        let about = match (picked, free.is_empty()) {
            (0, false) => fill(
                tr("{n} items, {free} available"),
                &[("n", &n), ("free", &free)],
            ),
            (0, true) => fill(tr("{n} items"), &[("n", &n)]),
            (k, false) => fill(
                tr("{k} of {n} selected, {free} available"),
                &[("k", &k), ("n", &n), ("free", &free)],
            ),
            (k, true) => fill(tr("{k} of {n} selected"), &[("k", &k), ("n", &n)]),
        };
        let about = fit(win, &about, inner, 11.5, None);
        g.labels.push(Label {
            centered: true,
            ..words(
                &about,
                (x + CARD_W / 2.0, about_y),
                inner,
                11.5,
                tones.dim_text,
            )
        });
        Some(g)
    }

    // ── what the chrome does ────────────────────────────────────────────

    /// The pointer came onto the sidebar at rest: in `SIDE_WAIT`, if it
    /// is still there, the sidebar opens.
    pub(super) fn filer_side_wait(&mut self, id: u32) {
        let timer = calloop::timer::Timer::from_duration(SIDE_WAIT);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if let Some(win) = app.filer.win_mut(id) {
                    if win.side_wait.take().is_some() && !win.pinned {
                        win.side_open = true;
                        app.filer_request_draw(id);
                    }
                }
                calloop::timer::TimeoutAction::Drop
            });
    }

    /// KEYBOARD MODE on or off, for every window, remembered.
    pub(super) fn filer_keys(&mut self, on: bool) {
        if self.settings.files_keys != on {
            self.settings.files_keys = on;
            self.settings.save();
            info!(
                "files: keyboard mode {}{}",
                if on { "on — " } else { "off" },
                if on { keys_words() } else { String::new() }
            );
        }
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            self.filer_request_draw(id);
        }
    }

    /// PIN the sidebar open beside the files, or let it rest again — every
    /// window, and kept.
    pub(super) fn filer_pin(&mut self, pinned: bool) {
        if self.settings.files_pinned != pinned {
            self.settings.files_pinned = pinned;
            self.settings.save();
            info!(
                "files: the sidebar {}",
                if pinned { "pinned open" } else { "rests" }
            );
        }
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            if let Some(win) = self.filer.win_mut(id) {
                win.pinned = pinned;
                win.side_open = pinned;
                win.side_t = if pinned { 1.0 } else { win.side_t };
                win.side_wait = None;
                win.names.1.clear();
            }
            self.filer_relayout(id);
            self.filer_hover(id);
        }
    }

    /// The current tab's state into its entry (before another is taken).
    fn filer_tab_keep(win: &mut Win) {
        let i = win.tab.min(win.tabs.len().saturating_sub(1));
        if let Some(t) = win.tabs.get_mut(i) {
            t.dir = win.dir.clone();
            t.back = win.back.clone();
            t.forward = win.forward.clone();
        }
    }

    /// A new tab on Home, right of the current one, taken.
    pub(super) fn filer_tab_new(&mut self, id: u32) {
        self.filer_tab_open(id, home());
    }

    /// A tab on `dir`, right of the current one ("Open in new tab").
    pub(super) fn filer_tab_open(&mut self, id: u32, dir: PathBuf) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        Self::filer_tab_keep(win);
        let tab = Tab::new(win.tab_seq, dir);
        win.tab_seq += 1;
        let at = (win.tab + 1).min(win.tabs.len());
        win.tabs.insert(at, tab);
        win.tab = at;
        self.filer_tab_show(id);
    }

    /// Take tab `n`.
    pub(super) fn filer_tab_take(&mut self, id: u32, n: usize) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if n >= win.tabs.len() || n == win.tab {
            return;
        }
        Self::filer_tab_keep(win);
        win.tab = n;
        self.filer_tab_show(id);
    }

    /// Close tab `n` (never the last: a window has one).
    pub(super) fn filer_tab_close(&mut self, id: u32, n: usize) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if win.tabs.len() < 2 || n >= win.tabs.len() {
            return;
        }
        if n != win.tab {
            win.tabs.remove(n);
            if n < win.tab {
                win.tab -= 1;
            }
            return self.filer_request_draw(id);
        }
        win.tabs.remove(n);
        win.tab = n.min(win.tabs.len() - 1);
        self.filer_tab_show(id);
    }

    /// Show the current tab: its folder, its way back and forward.
    fn filer_tab_show(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(t) = win.tabs.get(win.tab) else {
            return;
        };
        let dir = t.dir.clone();
        win.back = t.back.clone();
        win.forward = t.forward.clone();
        win.go_to = None;
        win.go_bad = false;
        self.filer_show(id, dir);
    }

    /// The sort menu, under its button: the key the things are ordered by,
    /// and which way.
    pub(super) fn filer_sort_menu(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let (key, desc) = win.sort;
        let key = [
            SortKey::Name,
            SortKey::Modified,
            SortKey::Size,
            SortKey::Kind,
        ]
        .iter()
        .position(|k| *k == key)
        .unwrap_or(0);
        let rows = crate::desktop_menu::sort_rows(key as u8, desc);
        let (w, h) = win.virt();
        let b = win.hits.sort;
        let ctx = crate::desktop_menu::Ctx::default();
        let mut menu = Menu::open(ctx, rows, None, false, (b.x, b.y + b.h), w, h);
        // (Its right edge under the button's: the menu opens leftward.)
        menu.hang((b.x + b.w, b.y + b.h + 4.0), false, true);
        win.selected.clear();
        win.menu = Some(menu);
        win.menu_path = Some(SORT_MENU.to_owned());
        win.props = None;
        win.hist_open = false;
        self.filer_request_draw(id);
    }

    /// A row of the sort menu.
    pub(super) fn filer_sort_pick(&mut self, id: u32, action: Action) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.sort = match action {
            Action::SortBy(n) => (
                [
                    SortKey::Name,
                    SortKey::Modified,
                    SortKey::Size,
                    SortKey::Kind,
                ][usize::from(n).min(3)],
                win.sort.1,
            ),
            Action::SortDir(desc) => (win.sort.0, desc),
            _ => return,
        };
        self.filer_list(id);
    }

    /// The i card wants what it says for the selection now: read off the
    /// loop (a folder's count, a disk's free space, the app that opens a
    /// kind) and drawn when it comes.
    pub(super) fn filer_info_due(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if !win.info_on {
            return;
        }
        let key = info_key(win);
        if win.info.as_ref().is_some_and(|i| i.key == key) || win.info_asked.as_ref() == Some(&key)
        {
            return;
        }
        win.info_asked = Some(key.clone());
        let dir = win.dir.clone();
        let picked: Vec<Item> = win
            .items
            .iter()
            .filter(|it| win.selected.contains(&it.path))
            .cloned()
            .collect();
        let here_icon = win
            .places
            .iter()
            .find(|p| p.path == dir)
            .map_or_else(|| "asset:folder".to_owned(), |p| p.icon.clone());
        let here_name = tab_name(&dir, &win.places);
        let shown = win.items.len();
        self.desktop_off_loop(
            move || read_info(key, &dir, &picked, here_icon, here_name, shown),
            move |app, (mut info, opens)| {
                let names = |id: &str| {
                    app.entries
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| e.name.clone())
                };
                if let Some(opens) = opens {
                    let name = names(&opens).unwrap_or_else(|| "—".to_owned());
                    info.rows.push((tr("Opens with").to_owned(), name));
                }
                let Some(win) = app.filer.win_mut(id) else {
                    return;
                };
                if win.info_asked.as_deref() == Some(info.key.as_str()) {
                    win.info_asked = None;
                }
                win.info = Some(info);
                app.filer_request_draw(id);
            },
        );
    }
}

impl App {
    /// THE HISTORY (`filer/history.rs`), under the ↶ and as far right as
    /// the tools go, in the menus' paint: a dim "History", then what was
    /// done here, newest first — its symbol, what it was, where and when,
    /// and its Undo (or "Undone", its words struck through, or "Can't
    /// undo"). As many rows as fit; the wheel shows the others.
    fn filer_history(
        &mut self,
        win: &mut Win,
        pal: &Palette,
        hits: &mut Hits,
    ) -> Option<GridContent> {
        hits.hist = Rect::default();
        hits.hist_undo.clear();
        hits.hist_rows.clear();
        if !win.hist_open || win.history.is_empty() {
            return None;
        }
        let (w, h) = win.virt();
        let top = HEAD_H / 2.0 + BTN_H / 2.0 + 4.0;
        let room = (h - top - 10.0).min(HIST_H) - 2.0 * HIST_PAD - HIST_HEAD;
        let len = win.history.len();
        let rows = ((room / HIST_ROW).floor().max(1.0) as usize).min(len);
        let first = (win.hist_first.max(0.0) as usize).min(len - rows);
        win.hist_first = win.hist_first.min((len - rows) as f32);
        // (What each row says, and what stands at its right: Undo, Undone,
        // Can't undo.)
        let shown: Vec<(u64, String, &'static str, String, bool, &'static str)> = (0..len)
            .rev()
            .skip(first)
            .take(rows)
            .map(|k| {
                let d = &win.history[k];
                let (right, button) = if d.done {
                    (tr("Undone"), false)
                } else if d.can {
                    (tr("Undo"), true)
                } else {
                    (tr("Can’t undo"), false)
                };
                let under = format!("{} · {}", d.place, super::history::ago(d.at));
                (d.n, d.text.clone(), d.sign, under, button, right)
            })
            .collect();
        let right_w = |win: &mut Win, words: &str, button: bool| {
            if button {
                measure(win, words, 11.5, None) + 18.0
            } else {
                measure(win, words, 11.0, None) + 8.0
            }
        };
        // As wide as its longest row wants, between the two widths.
        let mut want: f32 = 0.0;
        for (_, text, _, under, button, right) in &shown {
            let words = measure(win, text, 12.0, None).max(measure(win, under, 10.5, None));
            // (A few pixels to spare: the words are fitted with a little slack.)
            want = want
                .max(10.0 + 13.0 + 9.0 + words + 4.0 + 9.0 + right_w(win, right, *button) + 6.0);
        }
        let width = (want + 2.0 * HIST_PAD).clamp(HIST_MIN, HIST_MAX);
        let panel = Rect::new(
            w - 10.0 - width,
            top,
            width,
            2.0 * HIST_PAD + HIST_HEAD + rows as f32 * HIST_ROW,
        );
        hits.hist = panel;
        let paint = self.filer_panel_paint(panel, pal.ground);
        let fill = paint.fill;
        let ink = [paint.ink[0], paint.ink[1], paint.ink[2], 0.86];
        let dim = [paint.ink[0], paint.ink[1], paint.ink[2], 0.5];
        // (The menu's own fields: a step from what is under them, toward
        // black in the light, toward white in the dark.)
        let dark = 0.2126 * fill[0] + 0.7152 * fill[1] + 0.0722 * fill[2] < 0.18;
        let toward = if dark {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        let mut g = GridContent {
            clip: panel,
            ..Default::default()
        };
        // (Unlit, as every menu of a file window: `filer_panel_paint`.)
        g.rects.push(RectInst {
            glass: paint.glass,
            ..rect(panel, paint.radius, fill)
        });
        g.labels.push(Label {
            clip: Some(panel),
            ..words(
                tr("History"),
                (panel.x + HIST_PAD + 10.0, panel.y + HIST_PAD + 4.0),
                width - 2.0 * HIST_PAD - 20.0,
                11.0,
                dim,
            )
        });
        let mut y = panel.y + HIST_PAD + HIST_HEAD;
        for (n, text, sign, under, button, right) in shown {
            let row = Rect::new(panel.x + HIST_PAD, y, width - 2.0 * HIST_PAD, HIST_ROW);
            y += HIST_ROW;
            hits.hist_rows.push((row, n));
            let hot = matches!(win.hover, Spot::HistRow(m) | Spot::HistUndo(m) if m == n);
            // (What lies under the row's button: the panel, or the row
            // washed by the pointer.)
            let mut ground = fill;
            if hot {
                g.rects.push(rect(row, 5.0, paint.wash));
                let a = paint.wash[3];
                for (c, wc) in ground.iter_mut().zip(paint.wash).take(3) {
                    *c = *c * (1.0 - a) + wc * a;
                }
            }
            let cy = row.y + HIST_ROW / 2.0;
            symbol(
                &mut g,
                win,
                &glyph(sign),
                square((row.x + 10.0 + 6.5, cy), 13.0),
                mix(ground, paint.ink, 0.5),
            );
            // At the right: its Undo, or what stands in its place.
            let rw = right_w(win, right, button);
            let rx = row.x + row.w - 6.0 - rw;
            if button {
                let b = Rect::new(rx, cy - HIST_BTN_H / 2.0, rw, HIST_BTN_H);
                let pressed = win.hover == Spot::HistUndo(n);
                g.rects.push(rect(
                    b,
                    5.0,
                    mix(ground, toward, if pressed { 0.13 } else { 0.07 }),
                ));
                g.labels.push(Label {
                    clip: Some(panel),
                    ..words(
                        right,
                        (b.x + 9.0, cy - 15.5 / 2.0),
                        rw,
                        11.5,
                        [paint.ink[0], paint.ink[1], paint.ink[2], 1.0],
                    )
                });
                hits.hist_undo.push((b, n));
            } else {
                g.labels.push(Label {
                    clip: Some(panel),
                    ..words(right, (rx + 4.0, cy - 15.0 / 2.0), rw, 11.0, dim)
                });
            }
            // The words: what was done (struck through once taken back),
            // and under them where and when.
            let done = !button && right == tr("Undone");
            let tx = row.x + 10.0 + 13.0 + 9.0;
            let room = (rx - 9.0 - tx).max(20.0);
            let fitted =
                crate::desktop::fit_label(&text, room, &mut |t| measure(win, t, 12.0, None));
            let ty = row.y + 5.0;
            g.labels.push(Label {
                clip: Some(panel),
                ..words(
                    &fitted,
                    (tx, ty),
                    room + 2.0,
                    12.0,
                    if done { dim } else { ink },
                )
            });
            if done {
                let tw = measure(win, &fitted, 12.0, None).min(room);
                g.rects
                    .push(rect(Rect::new(tx, ty + 9.0, tw, 1.0), 0.0, dim));
            }
            let under =
                crate::desktop::fit_label(&under, room, &mut |t| measure(win, t, 10.5, None));
            g.labels.push(Label {
                clip: Some(panel),
                ..words(&under, (tx, ty + 17.0), room + 2.0, 10.5, dim)
            });
        }
        Some(g)
    }
}

// ── the selection pill ──────────────────────────────────────────────────

/// What a button of the selection pill does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PillAct {
    /// What the menu's row of that action does, on the selection.
    Act(Action),
    /// Opens the menu over the pill (on its Open with page).
    More,
    OpenWith,
    /// Lets go of the selection.
    Clear,
    /// The clipboard's files here; or forgotten.
    Paste,
    Forget,
    /// Keyboard mode's glyph: the shortcuts table.
    Keys,
    /// The × of "Keyboard driven": keyboard mode off (as Ctrl+K) — for
    /// whoever turned it on without meaning to (Max, 2026-10-22).
    KeysOff,
}

/// One part of the pill after its words: a rule, or a button (its symbol,
/// its words — none: a round one — what it does, and whether it can now).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PillPart {
    Rule,
    Button {
        icon: &'static str,
        words: String,
        act: PillAct,
        off: bool,
    },
}

/// What the pill shows: its words (the first strong, the rest dim), then
/// its parts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PillView {
    pub(crate) bold: String,
    pub(crate) dim: String,
    pub(crate) parts: Vec<PillPart>,
    /// Keyboard mode is on: its glyph at the pill's left end.
    pub(crate) keys: bool,
}

/// How long a finished thing is said in the pill.
pub(super) const FLASH: Duration = Duration::from_millis(2600);
/// The pill: its height, its air at the left (and the right), its words'
/// size, a button's height, a symbol, the most its strong words take.
const PILL_H: f32 = 34.0;
const PILL_LEFT: f32 = 14.0;
const PILL_RIGHT: f32 = 5.0;
const PILL_PX: f32 = 12.0;
const PILL_BTN: f32 = 24.0;
const PILL_ICON: f32 = 13.0;
const PILL_NAME: f32 = 220.0;
/// At most this many buttons with words (Max: "for kids and grandmas").
const PILL_WORDS: usize = 5;

/// What the pill says of the selection: “name” and its size, or "5
/// selected" and how many folders and how much.
fn pill_words(sel: &[&Item], meta: &HashMap<String, Meta>) -> (String, String) {
    let bytes: u64 = sel
        .iter()
        .filter(|it| it.kind == Kind::File)
        .map(|it| meta.get(&it.path).map_or(0, |m| m.size))
        .sum();
    let dirs = sel.iter().filter(|it| it.kind == Kind::Folder).count();
    let mut bits = Vec::new();
    if sel.len() > 1 && dirs > 0 {
        bits.push(if dirs == 1 {
            tr("1 folder").to_owned()
        } else {
            fill(tr("{n} folders"), &[("n", &dirs)])
        });
    }
    if bytes > 0 {
        bits.push(crate::desktop_props::size_text(bytes));
    }
    let first = match sel {
        [one] => format!("“{}”", one.name),
        _ => fill(tr("{n} selected"), &[("n", &sel.len())]),
    };
    (first, bits.join(" · "))
}

impl App {
    /// What window `win`'s selection pill shows now: a line said a moment
    /// ago; else the selection and what can be done with it; else the files
    /// on the clipboard, to be put here. Nothing: no pill.
    fn filer_pill_view(&self, win: &Win) -> Option<PillView> {
        if let Some((said, _)) = win.flash.as_ref().filter(|(_, at)| at.elapsed() < FLASH) {
            return Some(PillView {
                bold: said.clone(),
                dim: String::new(),
                parts: Vec::new(),
                keys: false,
            });
        }
        let button = |icon, words: &str, act| PillPart::Button {
            icon,
            words: words.to_owned(),
            act,
            off: false,
        };
        let round = |icon, act| PillPart::Button {
            icon,
            words: String::new(),
            act,
            off: false,
        };
        let sel: Vec<&Item> = win
            .items
            .iter()
            .filter(|it| win.selected.contains(&it.path))
            .collect();
        if !sel.is_empty() {
            let (bold, dim) = pill_words(&sel, &win.meta);
            let mut parts = vec![PillPart::Rule];
            // In the bin only what it is, and ×.
            if win.in_bin() == InBin::No {
                let one = sel.len() == 1;
                let file = one && sel[0].kind == Kind::File;
                let smart = self.filer_smart(win, &sel).bar;
                let mut words = Vec::new();
                if let Some((icon, label, action)) = smart.as_ref() {
                    parts.push(button(icon, label, PillAct::Act(*action)));
                    parts.push(PillPart::Rule);
                }
                // The system's (`zones`): copied and looked at, never changed.
                let theirs = sel.iter().any(|it| {
                    super::zones::zone_of(Path::new(&it.path)) == super::zones::Zone::System
                });
                words.push(button("copy", tr("Copy"), PillAct::Act(Action::Copy)));
                if !theirs {
                    words.push(button("move", tr("Move"), PillAct::Act(Action::Cut)));
                }
                if one && !theirs {
                    words.push(button("rename", tr("Rename"), PillAct::Act(Action::Rename)));
                }
                if file && smart.is_none() {
                    words.push(button("openwith", tr("Open with"), PillAct::OpenWith));
                }
                if !one && win.put_here() {
                    words.push(button(
                        "newf",
                        tr("New folder"),
                        PillAct::Act(Action::NewFolderWith),
                    ));
                }
                if let Some((stick, _)) = self.one_stick(Some(&win.dir)) {
                    let words_to = fill(tr("Copy to {stick}"), &[("stick", &stick)]);
                    words.push(button("usb", &words_to, PillAct::Act(Action::CopyToStick)));
                }
                words.truncate(PILL_WORDS - usize::from(smart.is_some()));
                parts.extend(words);
                if !theirs
                    && !sel
                        .iter()
                        .any(|it| crate::trash::protected(Path::new(&it.path)))
                {
                    parts.push(round("trash", PillAct::Act(Action::MoveToBin)));
                }
                parts.push(round("more", PillAct::More));
                parts.push(PillPart::Rule);
            }
            parts.push(round("x", PillAct::Clear));
            return Some(PillView {
                bold,
                dim,
                parts,
                keys: false,
            });
        }
        let (paths, cut) = self.clipboard_files()?;
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if self.filer.clip_forgot.as_ref() == Some(&names) {
            return None;
        }
        let bold = match paths.as_slice() {
            [one] => format!("“{}”", file_name(one)),
            _ => fill(tr("{n} items"), &[("n", &paths.len())]),
        };
        let (dim, icon, words) = if cut {
            (tr("to move"), "move", tr("Move here"))
        } else {
            (tr("copied"), "paste", tr("Copy here"))
        };
        Some(PillView {
            bold,
            dim: dim.to_owned(),
            parts: vec![
                PillPart::Rule,
                PillPart::Button {
                    icon,
                    words: words.to_owned(),
                    act: PillAct::Paste,
                    off: !win.put_here(),
                },
                PillPart::Rule,
                round("x", PillAct::Forget),
            ],
            keys: false,
        })
    }

    /// THE SELECTION PILL (Max, 2026-10-21, mockups v48–v49): floating 14 px
    /// above the foot of the files, centred on them, in the menus' paint
    /// with a hairline and a soft shadow; it rises and fades in. With a
    /// selection: what it is, then Copy · Move · Rename · Open with · New
    /// folder · Copy to <stick> (five with words at most), the bin, ⋯ (the
    /// right-click's menu, upward) and ×. After a Copy or a Move the files
    /// wait in it to be put somewhere ("3 items copied · Copy here · ×"); a
    /// finished thing is said in it for a moment.
    pub(super) fn filer_pill(
        &mut self,
        win: &mut Win,
        pal: &Palette,
        hits: &mut Hits,
        dt: f32,
        moving: &mut bool,
    ) -> Option<GridContent> {
        hits.pill = Rect::default();
        hits.pill_btns.clear();
        // KEYBOARD MODE shows in the pill (Max, 2026-10-22): its glyph at the
        // left of whatever the pill says — and, with nothing else to say,
        // "Keyboard driven".
        let keys = self.settings.files_keys;
        let view = match self.filer_pill_view(win) {
            Some(v) => Some(PillView { keys, ..v }),
            // ("agregale la x a la píldora por si un usuario lo activa sin
            // querer y no lo sabe desactivar": its × turns the mode off.)
            None if keys => Some(PillView {
                bold: tr("Keyboard driven").to_owned(),
                dim: String::new(),
                parts: vec![
                    PillPart::Rule,
                    PillPart::Button {
                        icon: "x",
                        words: String::new(),
                        act: PillAct::KeysOff,
                        off: false,
                    },
                ],
                keys: true,
            }),
            None => None,
        };
        win.pill_on = view.is_some();
        let (t, m) = crate::animation::ease_toward(
            win.pill_t,
            if view.is_some() { 1.0 } else { 0.0 },
            dt,
            TAB_RATE,
            0.004,
        );
        win.pill_t = t;
        *moving |= m;
        if view.is_some() {
            win.pill_view = view;
        } else if t < 0.01 {
            win.pill_view = None;
        }
        let view = win.pill_view.clone()?;
        let e = smooth(t);
        let (w, h) = win.virt();
        let left = win.field_left().max(win.side_now());
        // Its width: the words (the strong ones at most `PILL_NAME`), the
        // rules, the buttons.
        let px = PILL_PX;
        // (A name is cut at `PILL_NAME`; a line said alone has the room.)
        let name_max = if view.parts.is_empty() { w } else { PILL_NAME };
        let bold_w = measure(win, &view.bold, px, Some(crate::content::FONT_MEDIUM)).min(name_max);
        let dim_w = if view.dim.is_empty() {
            0.0
        } else {
            7.0 + measure(win, &view.dim, px, None)
        };
        let widths: Vec<f32> = view
            .parts
            .iter()
            .map(|p| match p {
                PillPart::Rule => 9.0,
                PillPart::Button { words, .. } if words.is_empty() => PILL_BTN,
                PillPart::Button { words, .. } => {
                    9.0 + PILL_ICON + 6.0 + measure(win, words, px, None) + 9.0
                }
            })
            .collect();
        let right = if view.parts.is_empty() {
            PILL_LEFT
        } else {
            PILL_RIGHT
        };
        let parts_w: f32 = widths.iter().sum::<f32>() + 2.0 * widths.len() as f32;
        // (The glyph's place, when it is there, is the words' air.)
        let lead = if view.keys {
            PILL_RIGHT + PILL_BTN + 4.0
        } else {
            PILL_LEFT
        };
        // (A few pixels to spare: the words are fitted with a little slack.)
        let words_w = bold_w + 4.0 + dim_w + if view.parts.is_empty() { 0.0 } else { 6.0 };
        let room = (w - left - 32.0).max(120.0);
        let width = (lead + words_w + parts_w + right).min(room);
        let cx = left + (w - left) / 2.0;
        let pill = Rect::new(
            (cx - width / 2.0).round(),
            (h - 14.0 - PILL_H + (1.0 - e) * 10.0).round(),
            width,
            PILL_H,
        );
        let paint = self.filer_panel_paint(pill, pal.ground);
        let fill = paint.fill;
        let ink = paint.ink;
        let dark = 0.2126 * fill[0] + 0.7152 * fill[1] + 0.0722 * fill[2] < 0.18;
        let toward = if dark {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        let a = |c: [f32; 4], k: f32| [c[0], c[1], c[2], c[3] * k * e];
        let mut g = GridContent {
            clip: Rect::new(pill.x - 30.0, pill.y - 20.0, pill.w + 60.0, pill.h + 50.0),
            ..Default::default()
        };
        // A soft shadow under it, its fill, a hairline.
        for k in 1..=5 {
            let k = k as f32;
            let grow = 2.4 * k;
            let s = Rect::new(
                pill.x - grow,
                pill.y - grow + 6.0,
                pill.w + 2.0 * grow,
                pill.h + 2.0 * grow,
            );
            g.rects.push(rect(
                s,
                PILL_H / 2.0 + grow,
                [0.0, 0.0, 0.0, 0.026 * (1.0 - k / 6.0) * e],
            ));
        }
        g.rects.push(RectInst {
            glass: paint.glass,
            ..rect(pill, PILL_H / 2.0, a(fill, 1.0))
        });
        // (Its lines and symbols as solid steps from its fill toward the
        // ink: laid on with a see-through ink they came out bright in the
        // dark.)
        let line = mix(fill, ink, 0.14);
        g.rects.push(RectInst {
            border: 1.0,
            ..rect(pill, PILL_H / 2.0, a(line, 1.0))
        });
        let cy = pill.y + PILL_H / 2.0;
        let text_y = cy - (px + 4.0) / 2.0;
        // The words: the first in full ink, a touch heavier where the font
        // has a medium (the mockup's 500 — never bold), fitted; the rest dim
        // after them.
        // Keyboard mode's glyph first: a round button, the shortcuts table.
        if view.keys {
            let r = Rect::new(pill.x + PILL_RIGHT, cy - PILL_BTN / 2.0, PILL_BTN, PILL_BTN);
            if win.hover == Spot::Pill(hits.pill_btns.len()) {
                g.rects
                    .push(rect(r, PILL_BTN / 2.0, a(mix(fill, toward, 0.07), 1.0)));
            }
            symbol(
                &mut g,
                win,
                &glyph("keyboard"),
                square(mid(r), PILL_ICON),
                a(mix(fill, ink, 0.55), 1.0),
            );
            hits.pill_btns.push((r, PillAct::Keys));
        }
        let mut x = pill.x + lead;
        let gap = if view.parts.is_empty() { 0.0 } else { 6.0 };
        let bold_room = (pill.w - lead - right - parts_w - dim_w - gap)
            .min(name_max + 4.0)
            .max(30.0);
        let shown = crate::desktop::fit_label(&view.bold, bold_room, &mut |t| {
            measure(win, t, px, Some(crate::content::FONT_MEDIUM))
        });
        g.labels.push(Label {
            clip: Some(pill),
            family: Some(crate::content::FONT_MEDIUM),
            cache: false,
            ..words(&shown, (x, text_y), bold_room + 2.0, px, a(ink, 1.0))
        });
        x += measure(win, &shown, px, Some(crate::content::FONT_MEDIUM)).min(bold_room);
        if !view.dim.is_empty() {
            x += 7.0;
            g.labels.push(Label {
                clip: Some(pill),
                cache: false,
                ..words(&view.dim, (x, text_y), dim_w + 2.0, px, a(ink, 0.5))
            });
            x += dim_w - 7.0;
        }
        x += 6.0;
        // The buttons, each its band under the pointer.
        for (part, bw) in view.parts.iter().zip(&widths) {
            match part {
                PillPart::Rule => {
                    g.rects.push(rect(
                        Rect::new(x + 4.0, cy - 7.0, 1.0, 14.0),
                        0.0,
                        a(line, 1.0),
                    ));
                }
                PillPart::Button {
                    icon,
                    words: label,
                    act,
                    off,
                } => {
                    let r = Rect::new(x, cy - PILL_BTN / 2.0, *bw, PILL_BTN);
                    let n = hits.pill_btns.len();
                    let k = if *off { 0.4 } else { 1.0 };
                    if win.hover == Spot::Pill(n) && !off {
                        g.rects
                            .push(rect(r, PILL_BTN / 2.0, a(mix(fill, toward, 0.07), 1.0)));
                    }
                    let icon_at = if label.is_empty() {
                        square(mid(r), PILL_ICON)
                    } else {
                        square((r.x + 9.0 + PILL_ICON / 2.0, cy), PILL_ICON)
                    };
                    symbol(
                        &mut g,
                        win,
                        &glyph(icon),
                        icon_at,
                        a(mix(fill, ink, 0.55), k),
                    );
                    if !label.is_empty() {
                        g.labels.push(Label {
                            clip: Some(pill),
                            cache: false,
                            ..words(
                                label,
                                (r.x + 9.0 + PILL_ICON + 6.0, text_y),
                                r.w,
                                px,
                                a(ink, k),
                            )
                        });
                    }
                    hits.pill_btns
                        .push((if *off { Rect::default() } else { r }, act.clone()));
                }
            }
            x += bw + 2.0;
        }
        if e > 0.5 {
            hits.pill = pill;
        }
        Some(g)
    }
}

/// The menu that is up is the sort menu (its `menu_path`).
pub(super) const SORT_MENU: &str = "\u{1}sort";

/// What the i card says (off the loop): of the things `picked`, else of the
/// folder `dir`; and the app that opens the one thing, by its id.
fn read_info(
    key: String,
    dir: &Path,
    picked: &[Item],
    here_icon: String,
    here_name: String,
    shown: usize,
) -> (Info, Option<String>) {
    let recents = dir == super::recent::root();
    let free =
        crate::desktop_props::space(if recents { home() } else { dir.to_path_buf() }.as_path())
            .map(|(_, free)| free);
    let count = |path: &Path| {
        std::fs::read_dir(path).map_or(0, |d| {
            d.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .count()
        })
    };
    let items = |n: usize| match n {
        1 => tr("1 item").to_owned(),
        n => fill(tr("{n} items"), &[("n", &n)]),
    };
    if picked.len() > 1 {
        let bytes: u64 = picked
            .iter()
            .filter(|it| it.kind == Kind::File)
            .filter_map(|it| std::fs::metadata(&it.path).ok())
            .map(|m| m.len())
            .sum();
        let info = Info {
            key,
            name: items(picked.len()),
            kind: if bytes > 0 {
                crate::desktop_props::size_text(bytes)
            } else {
                String::new()
            },
            rows: Vec::new(),
            icon: crate::desktop::asset_key("asset-doc"),
            free,
        };
        return (info, None);
    }
    // Recents, nothing picked: how many it lists.
    if recents && picked.is_empty() {
        let info = Info {
            key,
            name: here_name,
            kind: String::new(),
            rows: vec![(tr("Contains").to_owned(), items(shown))],
            icon: here_icon,
            free,
        };
        return (info, None);
    }
    let (path, name, kind, icon) = match picked.first() {
        Some(it) => (
            PathBuf::from(&it.path),
            it.name.clone(),
            it.kind,
            it.icon.clone(),
        ),
        None => (dir.to_path_buf(), here_name, Kind::Folder, here_icon),
    };
    let meta = std::fs::metadata(&path).ok();
    let mut rows = Vec::new();
    let mut opens = None;
    let what = match kind {
        Kind::Folder => {
            rows.push((tr("Contains").to_owned(), items(count(&path))));
            tr("Folder").to_owned()
        }
        Kind::Volume => {
            rows.push((tr("Contains").to_owned(), items(count(&path))));
            tr("Drive").to_owned()
        }
        Kind::Launcher => tr("Application").to_owned(),
        Kind::File => {
            if let Some(m) = &meta {
                let bytes = m.len();
                rows.push((
                    tr("Size").to_owned(),
                    fill(
                        tr("{size} ({bytes} bytes)"),
                        &[
                            ("size", &crate::desktop_props::size_text(bytes)),
                            ("bytes", &grouped(bytes)),
                        ],
                    ),
                ));
            }
            let lower = name.to_ascii_lowercase();
            let link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
            if !link {
                opens = crate::thumb_more::mime_of(&lower).and_then(crate::quicklook::default_app);
            }
            crate::desktop_props::file_kind(&name)
        }
    };
    if let Some(d) = crate::desktop_props::date_text(
        meta.and_then(|m| m.modified().ok())
            .filter(|t| *t > std::time::UNIX_EPOCH),
    ) {
        rows.push((tr("Modified").to_owned(), d));
    }
    if let Some(parent) = path.parent().filter(|_| !picked.is_empty()) {
        rows.push((tr("Where").to_owned(), parent.display().to_string()));
    } else if picked.is_empty() {
        rows.push((tr("Where").to_owned(), path.display().to_string()));
    }
    let info = Info {
        key,
        name,
        kind: what,
        rows,
        icon,
        free,
    };
    (info, opens.filter(|_| kind == Kind::File))
}

/// `2456789` as `2,456,789`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_grouped_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(2_456_789), "2,456,789");
    }

    /// The symbol names in `src` written right after `before` and right
    /// before `after`.
    fn names_between<'a>(src: &'a str, before: &str, after: &str) -> Vec<&'a str> {
        src.match_indices(before)
            .filter_map(|(at, _)| {
                let rest = &src[at + before.len()..];
                let end = rest.find(|c: char| !c.is_ascii_lowercase())?;
                (end > 0 && rest[end..].starts_with(after)).then(|| &rest[..end])
            })
            .collect()
    }

    /// Every symbol the top line, the pill and a kind's actions wear is
    /// one `glyphs.rs` has (one it has not is an empty gap by its words).
    #[test]
    fn every_symbol_worn_is_one_there_is() {
        let known: Vec<&str> = crate::glyphs::GLYPHS.iter().map(|(n, _)| *n).collect();
        let (chrome, smart) = (include_str!("chrome.rs"), include_str!("smart.rs"));
        let mut worn: Vec<&str> = CHROME.to_vec();
        for (before, after) in [("glyph(\"", "\")"), ("button(\"", "\""), ("round(\"", "\"")] {
            worn.extend(names_between(chrome, before, after));
        }
        for src in [chrome, smart] {
            worn.extend(names_between(src, "(\"", "\", tr("));
        }
        for name in [
            "print", "extract", "pic", "rotate", "shuffle", "play", "slides", "usb",
        ] {
            assert!(worn.contains(&name), "the scan missed {name}");
        }
        for name in worn {
            assert!(known.contains(&name), "no symbol {name}");
        }
    }

    /// Words under a panel drawn over them are left out — a word that only
    /// starts under it too — and the words beside it stay.
    #[test]
    fn words_under_a_panel_are_left_out() {
        let at = |text: &str, x: f32, y: f32| words(text, (x, y), 200.0, 12.0, [1.0; 4]);
        let mut labels = vec![
            at("Print", 100.0, 500.0),
            at("Copy", 300.0, 500.0),
            at("Rename", 560.0, 500.0),
            at("Open", 300.0, 300.0),
        ];
        // (Each letter 7 px wide.)
        let mut measure = |l: &Label| l.text.chars().count() as f32 * 7.0;
        // A menu reaching down over the middle of the pill.
        leave_out(
            &mut labels,
            Rect::new(280.0, 320.0, 300.0, 190.0),
            &mut measure,
        );
        let kept: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(kept, ["Print", "Open"]);
    }

    #[test]
    fn colours_are_mixed_as_a_page_mixes_them() {
        let (white, black) = ([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
        // (The mockup's black at 42 % over white: #949494.)
        let m = mix(white, black, 0.42);
        assert!((srgb(m[0]) - 0.58).abs() < 0.002);
        assert_eq!(mix(white, black, 0.0), white);
    }
}

//! OPTIONS content: the modular "pills" on the topbar and their behaviour.
//!
//! OPTIONS is the context-aware layer that lives on the topbar (see
//! [`crate::screencopy`] for the bar's diegetic colour-matching). Its UI is
//! built from independent **pill** modules; which ones show depends on context.
//! For now: a clock pill (far right), the focused window's name (centre) with
//! the red close button always beside it on the right; the window-mode
//! toggles (pseudotile, fullscreen) hide to the close's right and reveal
//! when the name or close is hovered. (Floating was cut 2026-08-31 — Golem
//! has no drag-a-titlebar story, and pseudo covers "own size, in place".)
//!
//! Text pills use the **dock's font** (the default SansSerif — DejaVu Sans);
//! icon pills use a **Nerd Font**. Backgrounds are transparent, brightening on
//! hover; a pill holding a single glyph is a perfect circle (radius = height/2
//! makes every pill a stadium, which is a circle when width == height).
//! Proportional text means pill widths are measured (cached) rather than
//! estimated.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::animation::{ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene, ShadowInst};
use crate::{hypr, surface, App, BTN_LEFT, BTN_RIGHT};

/// Thickness of the top reveal strip (logical px) — the only pointer-sensitive
/// band while the bar is hidden in fullscreen.
const REVEAL_PX: f32 = 3.0;
/// How long the pointer must dwell at the top edge before the bar reveals — a
/// deliberate hold so it only happens when really intended.
const REVEAL_DWELL: Duration = Duration::from_millis(1000);
/// Grace after the pointer leaves the revealed bar before it conceals again.
const HIDE_GRACE: Duration = Duration::from_millis(500);

/// Nerd Font for the icon glyphs (close / pseudotile). Shared with the
/// notification OPTION ([`crate::notif`]) for its bell glyph.
pub(crate) const NERD: &str = "JetBrainsMono Nerd Font Mono";
/// Text (clock, window name) uses `None` → the default SansSerif, which is
/// exactly the font the dock uses (fontconfig resolves it to DejaVu Sans).
const TEXT_FONT: Option<&str> = None;
pub(crate) const FONT_PX: f32 = 17.0;
pub(crate) const LINE_PX: f32 = 20.0;

/// Margin above/below the pills — leaves room for the neumorphic rim so the
/// pills themselves stay compact rather than filling the whole bar.
pub(crate) const PILL_MARGIN_Y: f32 = 2.5;
pub(crate) const PILL_PAD_X: f32 = 11.5;
pub(crate) const EDGE_PAD: f32 = 6.0;
/// Gaps between the window pill and the controls, and between the two control
/// circles — 3px, so each button keeps its full round outline (and its rim).
pub(crate) const GROUP_GAP: f32 = 3.0;
const CTRL_GAP: f32 = 3.0;
/// The two OPTIONS separation scales. Bonded parts of a *single* OPTION (the
/// fixed bell and the preview that slides out from behind it) hug close so they
/// read as one unit; *distinct* OPTIONS (the notification OPTION vs the clock)
/// sit further apart so the boundary between them is legible.
pub(crate) const BOND_GAP: f32 = 3.0;
pub(crate) const OPTION_GAP: f32 = 9.0;
/// Unified pill hover lift: every hoverable pill grows this much per side while
/// hovered, so the feedback is one consistent, tactile effect across the bar
/// (paired with the stronger hover wash). The Notif bell is exempt — its hover
/// response is the peek metamorphosis, not a lift.
pub(crate) const PILL_HOVER_GROW: f32 = 2.0;
const TITLE_MAX: usize = 48;

// Nerd Font glyphs (Font Awesome range, present in JetBrainsMono NF).
pub(crate) const GLYPH_CLOSE: &str = "\u{f00d}"; // fa-times
const GLYPH_SQUARE: &str = "\u{f096}"; // fa-square-o (pseudotile)
const GLYPH_FULL: &str = "\u{f065}"; // fa-expand (fullscreen)
pub(crate) const GLYPH_BELL: &str = "\u{f0f3}"; // fa-bell (notification OPTION)
pub(crate) const GLYPH_BELL_SLASH: &str = "\u{f1f6}"; // fa-bell-slash (mute pill)
pub(crate) const GLYPH_CLIPBOARD: &str = "\u{f0ea}"; // fa-clipboard (clipboard OPTION)
pub(crate) const GLYPH_COPY: &str = "\u{f0c5}"; // fa-copy (detail-view copy)
pub(crate) const GLYPH_COPY_LINK: &str = "\u{f0c1}"; // fa-link (copy the page URL)

// Pill backgrounds (resting + hover) are adaptive washes — see
// `options_rest_wash` / `options_hover_wash`.

// --- Control-button reveal animation ---------------------------------------
// The mode toggles are hidden by default (close is NOT — it rests beside the
// window name, always visible); hovering the window pill or the close button
// makes the toggles slide out rightward from behind their parent pill,
// staggered (pseudo from behind the close, then fullscreen from behind
// pseudo). Leaving plays the same slide backward — the chain retracts
// outermost-first, each tucking back behind its parent while it fades. One
// progress value per button drives both position and opacity, the same
// metamorphosis feel as the copy-link pill and the bell peek. All dt-based.
const CTRL_STAGGER: f32 = 0.06; // s between stagger stages
/// Glide rate of a toggle's progress (exponential approach) — matches the
/// bar's other metamorphoses (`MORPH_RATE`).
const CTRL_RATE: f32 = 13.0;
const CTRL_EPS: f32 = 0.002;
/// How many toggles ride the reveal chain.
const CTRL_N: usize = 2;

/// Per-button progress for the reveal animation. Buttons are ordered
/// [pseudo, fullscreen] (see [`ctrl_index`]); close is not animated — it is
/// always at rest.
#[derive(Debug, Default)]
pub(crate) struct CtrlAnim {
    /// Whether the cluster should be revealed (pointer on the window/cluster).
    reveal: bool,
    /// When `reveal` last flipped — drives the stagger in both directions.
    changed_at: Option<std::time::Instant>,
    /// Slide progress 0 (tucked behind the parent) → 1 (resting); opacity is
    /// derived from it, so the hide plays the slide backward.
    t: [f32; CTRL_N],
    last: Option<std::time::Instant>,
    frame_pending: bool,
}

// --- Live resize readout ----------------------------------------------------
// The window pill shows the focused window's size (`1992 × 1199`) instead of
// its title whenever the pointer could grab a border — the same geometric
// band the compositor uses to flip the cursor to a resize arrow — and while
// the size is actually changing (a drag in progress, a keyboard resize).
// Detection is a poll keyed by window address (the compositor emits no
// drag/resize/cursor events; same precedent as the intellihide zone poll):
// slow at rest, fast while the readout is up so the numbers track the drag.
/// Sampling interval at rest (a `j/activewindow` + `j/cursorpos` read).
pub(crate) const SIZE_POLL_SLOW: Duration = Duration::from_millis(250);
/// Sampling interval while the readout is up — fast enough to read as a
/// continuous counter tracking the drag.
const SIZE_POLL_FAST: Duration = Duration::from_millis(40);
/// How long the size must hold still (with the pointer off the border)
/// before the pill reverts to the title.
const SIZE_HOLD: Duration = Duration::from_millis(700);
/// Half-width of the border grab band, matching the compositor's resize
/// cursor: `general.border_size` (3) + `extend_border_grab_area` (10), plus
/// a pixel of slack. Within this distance of the window's edge ring, the
/// pointer shows a resize arrow — and the pill shows the size.
const BORDER_BAND: i32 = 14;

// --- Clock↔date metamorphosis ----------------------------------------------
// Hovering the clock pill grows it horizontally and crossfades HH:MM into the
// full date; it holds as the date for a few seconds after the pointer leaves,
// then plays the same transition backwards. All dt-based.
/// Glide rate of the metamorphosis progress (exponential approach).
const META_RATE: f32 = 13.0;
const META_EPS: f32 = 0.001;
/// How long the pill stays on the date after the hover leaves.
const META_HOLD: Duration = Duration::from_millis(1500);
/// Crossfade split: the clock fades out by `t = OUT_END`, the date fades in
/// from `t = IN_START` — a slight overlap in the middle keeps it smooth.
const META_OUT_END: f32 = 0.55;
const META_IN_START: f32 = 0.45;

/// Progress + timing state for the clock↔date metamorphosis.
#[derive(Debug, Default)]
pub(crate) struct ClockMeta {
    /// Whether the pill should show the date (pointer on it, or within hold).
    reveal: bool,
    /// Progress 0 (clock) → 1 (date).
    t: f32,
    last: Option<std::time::Instant>,
    frame_pending: bool,
    /// When the post-leave hold expires and the pill collapses back to clock.
    hold_deadline: Option<std::time::Instant>,
}

/// Animation slot for a mode-toggle pill (`None` for window/clock/close —
/// close is a resting pill, always visible beside the window name).
fn ctrl_index(id: PillId) -> Option<usize> {
    match id {
        PillId::Pseudo => Some(0),
        PillId::Fullscreen => Some(1),
        _ => None,
    }
}

/// Back-to-front draw order so each parent pill occludes the control emerging
/// from behind it: fullscreen ← pseudo ← close. (Window and clock are
/// independent resting pills — they never overlap the emerge chain.)
fn draw_z(id: PillId) -> u8 {
    match id {
        PillId::Fullscreen => 1,
        PillId::Pseudo => 2,
        PillId::Close => 3,
        PillId::Window => 4,
        PillId::Clock => 5,
        // The preview/box (Notif) draws first; the fixed bell (NotifMute) draws
        // on top of it, capping its right end as it grows out from behind.
        PillId::Notif => 6,
        PillId::NotifMute => 7,
        // Mirror of the bell on the left edge: the box + copy-link pill draw
        // first (emerging from behind), then the small fixed glyph pill on top.
        PillId::ClipboardBox => 8,
        PillId::ClipCopyLink => 8,
        PillId::Clipboard => 9,
    }
}

// A single, tiny soft shadow around the button circles for a touch of depth —
// black when the bar is bright, white when it's dark (never both). Strengths
// are display-meaningful (the white is gamma-corrected like the pill wash).
const NEU_BLUR: f32 = 3.5;
const NEU_DARK: f32 = 0.24; // black shadow on a bright bar
const NEU_LIGHT: f32 = 0.11; // white shadow on a dark bar

/// Which surface the pointer is over (only `Enter` carries the surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerSurface {
    Dock,
    Options,
}

/// The pill modules currently on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PillId {
    Clock,
    /// The notification OPTION — a bell that metamorphoses on hover (see
    /// [`crate::notif`]).
    Notif,
    /// The mute-notifications pill: rests directly *behind* the bell and is
    /// uncovered (to the bell's right) as the bell slides left to peek open.
    NotifMute,
    /// The clipboard OPTION's small fixed pill: a clipboard glyph at the left
    /// edge. The preview/history box slides out to its right from behind it —
    /// the left-side mirror of the bell + its box (see [`crate::clipboard`]).
    Clipboard,
    /// The clipboard OPTION's morphing preview/history box (rests behind the
    /// small pill, slides out rightward). Mirrors `Notif`.
    ClipboardBox,
    /// The "copy link" pill that slides out from behind the small clipboard pill
    /// when the focused app is a browser; a click copies its current page URL.
    ClipCopyLink,
    Window,
    Close,
    Pseudo,
    Fullscreen,
}

struct Pill {
    id: PillId,
    rect: Rect,
    text: String,
    /// Font family (`None` = the dock's SansSerif; `Some` for the Nerd icons).
    family: Option<&'static str>,
    /// Glyph colour override (`None` = default text colour).
    glyph_color: Option<[f32; 4]>,
}

/// Local time as `HH:MM`, via libc so it respects the timezone.
fn clock_now() -> String {
    // SAFETY: `localtime_r` fills a caller-owned `tm`; `time` takes null.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// Local date as `Weekday, D Month YYYY` (e.g. "Friday, 31 July 2026"), via
/// libc so it respects the timezone.
fn date_now() -> String {
    const WD: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const MO: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    // SAFETY: `localtime_r` fills a caller-owned `tm`; `time` takes null.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let wd = WD[(tm.tm_wday as usize).min(6)];
        let mo = MO[(tm.tm_mon as usize).min(11)];
        format!("{wd}, {} {mo} {}", tm.tm_mday, tm.tm_year + 1900)
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Build a pill wash whose alpha `a` means its true **on-screen** strength.
///
/// The bar's swapchain is an sRGB surface and the renderer outputs premultiplied
/// colour, so a plain white overlay's RGB is gamma-lifted far above its alpha —
/// a low-alpha white wash looks much stronger than the number implies (e.g.
/// `0.04` shows as ~22% grey on black). Pre-dividing the linearised alpha back
/// out of the RGB cancels that, so `a` becomes the actual displayed fraction.
/// Black needs no correction (0 stays 0 through the encode).
/// Grow a pill's drawn rect outward from its centre by the unified hover lift
/// ([`PILL_HOVER_GROW`]). Only the drawn geometry grows; layout and hit rects
/// are unchanged.
pub(crate) fn hover_grow(rect: Rect) -> Rect {
    Rect::new(
        rect.x - PILL_HOVER_GROW,
        rect.y - PILL_HOVER_GROW,
        rect.w + 2.0 * PILL_HOVER_GROW,
        rect.h + 2.0 * PILL_HOVER_GROW,
    )
}

pub(crate) fn wash(white: bool, a: f32) -> [f32; 4] {
    if white && a > 0.0 {
        let v = srgb_to_linear(a) / a;
        [v, v, v, a]
    } else {
        [0.0, 0.0, 0.0, a]
    }
}

/// Push a single tiny soft shadow around a circle for a touch of depth: black
/// on a bright bar, white on a dark one (a uniform exterior penumbra). The
/// white is gamma-corrected so its strength means its true on-screen level.
pub(crate) fn push_neumorph(scene: &mut Scene, rect: Rect, radius: f32, bright: bool, alpha: f32) {
    let color = if bright {
        [0.0, 0.0, 0.0, NEU_DARK * alpha]
    } else {
        let v = srgb_to_linear(NEU_LIGHT) / NEU_LIGHT;
        [v, v, v, NEU_LIGHT * alpha]
    };
    scene.overlay_shadows.push(ShadowInst {
        rect,
        radius,
        blur: NEU_BLUR,
        color,
        edges: [1.0, 1.0, 1.0, 1.0], // uniform soft penumbra all around
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

impl App {
    /// Startup clock value, so the pill shows immediately.
    pub(crate) fn options_clock_init() -> String {
        clock_now()
    }

    /// Startup date value (shown when the clock pill is hovered).
    pub(crate) fn options_date_init() -> String {
        date_now()
    }

    /// Compute the current pills (interactive modules) for the bar state.
    fn options_pills(&self) -> Vec<Pill> {
        let w = self.options_size.0 as f32;
        let bar_h = self.config.options.height as f32;
        if w == 0.0 {
            return Vec::new();
        }
        // Pills fill almost the whole bar height, top to bottom.
        let ph = (bar_h - 2.0 * PILL_MARGIN_Y).max(1.0);
        let y = PILL_MARGIN_Y;
        let mut pills = Vec::new();

        // Clock, far right. Its width metamorphoses between the HH:MM and the
        // full date as the pill is hovered; it grows leftward (right edge
        // pinned at the bar edge). The wider rect stays hoverable-as-clock, so
        // the date holds open while the pointer is over it.
        let mut clock_left = w - EDGE_PAD;
        if !self.options_clock.is_empty() {
            let content_w = lerp(
                self.options_clock_w,
                self.options_date_w,
                self.options_clock_meta.t,
            );
            let cw = (content_w + 2.0 * PILL_PAD_X).max(ph);
            clock_left = w - EDGE_PAD - cw;
            pills.push(Pill {
                id: PillId::Clock,
                rect: Rect::new(clock_left, y, cw, ph),
                text: self.options_clock.clone(),
                family: TEXT_FONT,
                glyph_color: None,
            });
        }

        // Notification OPTION: one element (bell → preview pill → history
        // rectangle) just left of the clock; its full drawing + morph is in
        // `crate::notif`. Its rect is the whole morphing shape (it grows *down*
        // past the bar when expanded), right edge pinned a gap left of the clock.
        pills.push(Pill {
            id: PillId::Notif,
            rect: self.notif_geom(clock_left - OPTION_GAP, y, ph),
            text: GLYPH_BELL.to_owned(),
            family: Some(NERD),
            glyph_color: None,
        });

        // Mute pill: a fixed circle in the bell's *original* resting slot (right
        // edge a gap left of the clock). At rest the bell sits exactly on top of
        // it; as the bell peeks open it slides left (see `notif_geom`) and
        // uncovers this pill to its right. Drawn under the bell (see `draw_z`).
        pills.push(Pill {
            id: PillId::NotifMute,
            rect: Rect::new(clock_left - OPTION_GAP - ph, y, ph, ph),
            text: GLYPH_BELL_SLASH.to_owned(),
            family: Some(NERD),
            glyph_color: None,
        });

        // Clipboard OPTION: the left-edge mirror of the notification cluster.
        // A small fixed glyph pill sits at the left edge; the preview/history box
        // slides out to its RIGHT from behind it (drawn first, capped by the
        // small pill on top). `crate::clipboard` draws the box.
        pills.push(Pill {
            id: PillId::ClipboardBox,
            rect: self.clip_geom(EDGE_PAD, y, ph),
            text: String::new(),
            family: None,
            glyph_color: None,
        });
        pills.push(Pill {
            id: PillId::Clipboard,
            rect: Rect::new(EDGE_PAD, y, ph, ph),
            text: GLYPH_CLIPBOARD.to_owned(),
            family: Some(NERD),
            glyph_color: None,
        });

        // Copy-link pill: slides out from behind the small clipboard pill to its
        // right when the focused app is a browser (has a copyable page URL).
        let lt = self.clip_link_t();
        if lt > 0.01 {
            let out_x = EDGE_PAD + ph + crate::clipboard::LINK_GAP;
            pills.push(Pill {
                id: PillId::ClipCopyLink,
                rect: Rect::new(lerp(EDGE_PAD, out_x, lt), y, ph, ph),
                text: GLYPH_COPY_LINK.to_owned(),
                family: Some(NERD),
                glyph_color: None,
            });
        }

        // The window name pill is centred *alone* (so it doesn't shift when the
        // toggles reveal); close rests beside it, ALWAYS visible; the mode
        // toggles hide until hover, at fixed resting spots right of the close:
        //   [window name] [X] [pseudo] [fullscreen]
        if self.options_title.is_some() {
            // Title, with the live resize readout appended while active.
            let shown = self.options_window_text().unwrap_or_default();
            let ww = (self.options_title_w + 2.0 * PILL_PAD_X).max(ph);
            let d = ph; // control-circle diameter
            let wx = ((w - ww) / 2.0).max(EDGE_PAD);
            let circle = |pills: &mut Vec<Pill>, x: f32, id, glyph: &str, color| {
                pills.push(Pill {
                    id,
                    rect: Rect::new(x, y, d, d),
                    text: glyph.to_owned(),
                    family: Some(NERD),
                    glyph_color: color,
                });
            };
            pills.push(Pill {
                id: PillId::Window,
                rect: Rect::new(wx, y, ww, ph),
                text: shown,
                family: TEXT_FONT,
                glyph_color: None,
            });
            // Close, right of the window name — a resting pill, no reveal.
            let close_x = wx + ww + GROUP_GAP;
            circle(&mut pills, close_x, PillId::Close, GLYPH_CLOSE, None);
            // Window-mode toggles, right of the close (pseudo nearest,
            // fullscreen outermost).
            let mut cx = close_x + d + GROUP_GAP;
            circle(&mut pills, cx, PillId::Pseudo, GLYPH_SQUARE, None);
            cx += d + CTRL_GAP;
            circle(&mut pills, cx, PillId::Fullscreen, GLYPH_FULL, None);
        }
        pills
    }

    /// The clock pill's current left edge (accounting for its date
    /// metamorphosis), or the bar's right padding when there's no clock — the
    /// anchor the notification element pins its right edge a gap left of.
    pub(crate) fn options_clock_left(&self) -> f32 {
        let w = self.options_size.0 as f32;
        if self.options_clock.is_empty() {
            return w - EDGE_PAD;
        }
        let bar_h = self.config.options.height as f32;
        let ph = (bar_h - 2.0 * PILL_MARGIN_Y).max(1.0);
        let content_w = lerp(
            self.options_clock_w,
            self.options_date_w,
            self.options_clock_meta.t,
        );
        let cw = (content_w + 2.0 * PILL_PAD_X).max(ph);
        w - EDGE_PAD - cw
    }

    /// The window pill's text: the (truncated) title, with the live size
    /// appended — `current task (342x343)` — while the pointer is on a
    /// resize border or a resize is in flight.
    fn options_window_text(&self) -> Option<String> {
        let title = self.options_title.as_ref().map(|t| truncate(t, TITLE_MAX))?;
        Some(match self.options_resize_live {
            Some((w, h)) => format!("{title} ({w}x{h})"),
            None => title,
        })
    }

    /// Re-measure the clock + window-title text widths (proportional font, so
    /// widths must be measured, not estimated). Cheap; only on data change.
    pub(crate) fn measure_options_text(&mut self) {
        let clock = self.options_clock.clone();
        let date = self.options_date.clone();
        let title = self.options_window_text();
        let Some(r) = self.options_renderer.as_mut() else {
            return;
        };
        let cw = r.measure_text(&clock, FONT_PX, TEXT_FONT);
        let dw = r.measure_text(&date, FONT_PX, TEXT_FONT);
        let tw = title
            .as_deref()
            .map_or(0.0, |t| r.measure_text(t, FONT_PX, TEXT_FONT));
        self.options_clock_w = cw;
        self.options_date_w = dw;
        self.options_title_w = tw;
    }

    /// Whether the matched bar is bright enough to want dark text/ink.
    /// (`options_bar_matched` is stored linear, so this is true relative
    /// luminance; 0.179 is the WCAG flip point where black and white contrast
    /// equally.) A transparent bar counts as dark.
    pub(crate) fn options_bar_is_bright(&self) -> bool {
        match self.options_bar_matched {
            Some(c) => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2] > 0.179,
            None => false,
        }
    }

    /// Adaptive default text colour: black on a bright matched bar, white on a
    /// dark one, so the pills stay legible against the window they blend into.
    /// Falls back to the theme colour when the bar is transparent.
    pub(crate) fn options_text_color(&self) -> [f32; 4] {
        match self.options_bar_matched {
            Some(_) if self.options_bar_is_bright() => [0.0, 0.0, 0.0, 1.0],
            Some(_) => [1.0, 1.0, 1.0, 1.0],
            None => self.config.theme.text_rgba(),
        }
    }

    /// Resting pill background — adaptive to the bar's brightness (same
    /// detector as the text colour). The alphas are **asymmetric on purpose**:
    /// white-on-dark reads ~2–3× stronger than black-on-white at equal alpha
    /// (we're far more sensitive to light added to darkness), so the white
    /// wash must be much lighter to feel as subtle as the black one.
    pub(crate) fn options_rest_wash(&self) -> [f32; 4] {
        if self.options_bar_is_bright() {
            wash(false, 0.10)
        } else {
            wash(true, 0.11)
        }
    }

    /// Hover wash — stronger than the resting wash, with the same asymmetry.
    pub(crate) fn options_hover_wash(&self) -> [f32; 4] {
        if self.options_bar_is_bright() {
            wash(false, 0.30)
        } else {
            wash(true, 0.27)
        }
    }

    /// Add the OPTIONS pills to the bar's scene (called after the base fill).
    /// Control buttons carry the reveal animation: a horizontal slide from
    /// behind their parent pill (offset from `slide`) plus an opacity from
    /// `alpha`; window/clock are always at rest, full opacity.
    pub(crate) fn push_options_pills(&self, scene: &mut Scene) {
        let hover_wash = self.options_hover_wash();
        let rest_wash = self.options_rest_wash();
        let bright = self.options_bar_is_bright();
        let text_color = self.options_text_color();
        let bar_h = self.config.options.height as f32;
        let full_w = self.options_size.0 as f32;

        let pills = self.options_pills();
        // Resting rect of a pill by id, for computing where each control is
        // tucked (behind its parent) and the edge it emerges past.
        let home = |id: PillId| pills.iter().find(|p| p.id == id).map(|p| p.rect);
        // Draw parents last so they occlude the buttons emerging behind them.
        let mut order: Vec<&Pill> = pills.iter().collect();
        order.sort_by_key(|p| draw_z(p.id));

        for pill in order {
            // The notification OPTION draws itself (bell ↔ peek metamorphosis).
            if pill.id == PillId::Notif {
                self.push_notif_pill(scene, pill.rect);
                continue;
            }
            // The bell (fixed DND toggle) is always drawn, on top of the sliding
            // preview/box that grows out from behind it.
            if pill.id == PillId::NotifMute {
                self.push_notif_mute(scene, pill.rect);
                continue;
            }
            // The clipboard box draws itself (slides out from behind the small
            // pill); the small glyph pill draws on top of it, with its own
            // fresh-clip beat (like the bell's muted-arrival blink).
            if pill.id == PillId::ClipboardBox {
                self.push_clip_pill(scene, pill.rect);
                continue;
            }
            if pill.id == PillId::Clipboard {
                self.push_clip_glyph(scene, pill.rect);
                continue;
            }
            if pill.id == PillId::ClipCopyLink {
                self.push_clip_link(scene, pill.rect, &pill.text);
                continue;
            }
            // Reveal animation for the control buttons: slide out horizontally
            // from behind the parent's near edge (slide 0 = tucked, 1 = rest),
            // fading in; the glyph is clipped to the emerge side so it reads as
            // coming out from under the parent rather than through it.
            let (rect, a, clip, shadow_a) = match ctrl_index(pill.id) {
                Some(i) => {
                    let t = self.options_ctrl.t[i];
                    // Opacity rides the slide (the same mapping as the
                    // copy-link pill), so show and hide are one symmetric
                    // metamorphosis — the hide tucks back while fading.
                    let a = ((t - 0.15) / 0.6).clamp(0.0, 1.0);
                    if a <= 0.01 {
                        continue; // fully hidden — don't draw
                    }
                    let d = pill.rect.w;
                    // `origin` = tucked-x behind the parent; `edge` = the
                    // vertical line the glyph emerges past. Mode toggles
                    // emerge rightward, each from behind the previous pill's
                    // right edge, starting at the close (which never gets
                    // here — it has no ctrl slot).
                    let (origin, edge) = match pill.id {
                        PillId::Pseudo => {
                            let cr = home(PillId::Close).map_or(pill.rect.x + d, |r| r.x + r.w);
                            (cr - d, cr)
                        }
                        PillId::Fullscreen => {
                            let pr = home(PillId::Pseudo).map_or(pill.rect.x, |r| r.x + r.w);
                            (pr - d, pr)
                        }
                        _ => (pill.rect.x, pill.rect.x),
                    };
                    let x = lerp(origin, pill.rect.x, t);
                    let rect = Rect::new(x, pill.rect.y, d, pill.rect.h);
                    let clip = Rect::new(edge, 0.0, (full_w - edge).max(0.0), bar_h);
                    // Gate the depth shadow by slide so a tucked button's halo
                    // doesn't leak over the parent (overlay shadows draw on top).
                    (rect, a, Some(clip), a * t)
                }
                None => (pill.rect, 1.0, None, 1.0),
            };
            // Unified hover lift: a hovered pill grows a touch (drawn geometry
            // only) so hover reads as a tactile rise, not a shrink.
            let hovered = self.options_hover == Some(pill.id);
            let rect = if hovered { hover_grow(rect) } else { rect };
            let radius = rect.h / 2.0; // stadium ⇒ circle when w == h
            push_neumorph(scene, rect, radius, bright, shadow_a);
            let base = if hovered { hover_wash } else { rest_wash };
            scene.rects.push(RectInst {
                rect,
                radius,
                color: [base[0], base[1], base[2], base[3] * a],
                glass: 0.0,
            });
            let g = pill.glyph_color.unwrap_or(text_color);
            let family = pill.family;
            let cx = rect.x + rect.w / 2.0;
            let ty = rect.y + (rect.h - LINE_PX) / 2.0;
            let mk = |text: String, alpha: f32, max_w: f32, clip: Option<Rect>| Label {
                text,
                pos: (cx, ty),
                max_w,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family,
                color: Some([g[0], g[1], g[2], g[3] * alpha]),
                clip,
            };
            // The clock pill crossfades HH:MM ↔ the full date during its
            // metamorphosis: the clock fades out early, the date fades in late
            // (a slight overlap), the date centred on the pill and scissor-
            // clipped to it so it reveals from the centre outward as it grows.
            let t = self.options_clock_meta.t;
            if pill.id == PillId::Clock && t > 0.001 {
                let out = (1.0 - t / META_OUT_END).clamp(0.0, 1.0);
                let inn = ((t - META_IN_START) / (1.0 - META_IN_START)).clamp(0.0, 1.0);
                if out > 0.001 {
                    scene
                        .labels
                        .push(mk(self.options_clock.clone(), out, rect.w, Some(rect)));
                }
                if inn > 0.001 {
                    scene.labels.push(mk(
                        self.options_date.clone(),
                        inn,
                        self.options_date_w + 2.0,
                        Some(rect),
                    ));
                }
            } else {
                scene.labels.push(mk(pill.text.clone(), 1.0, rect.w, clip));
            }
        }
    }

    /// Refresh the focused-window pill (title + address) on layout changes
    /// and on Brain snapshots.
    ///
    /// Data source: the Brain's context snapshot when its compositor layer
    /// is alive (event-driven, no socket round-trips — the S2 Spine's first
    /// consumer); otherwise the original direct `hyprctl` poll, so the pill
    /// keeps working if the engine goes dark (degrade path).
    pub(crate) fn refresh_options_content(&mut self) {
        if self.options_layer.is_none() {
            return;
        }
        // Overview-aware: while waveview owns the screen the pill's focus IS
        // the overview — no window address (the control pills stand down)
        // and no fullscreen conceal.
        if self.overview_active {
            let title = Some("Overview".to_owned());
            if self.options_active_addr.is_some() || self.options_title != title {
                self.options_active_addr = None;
                self.options_title = title;
                self.set_clip_link_available(false);
                self.measure_options_text();
                self.sync_options_input();
                self.draw_options();
            }
            return;
        }
        let (addr, title, class, fullscreen) = match self.brain.as_ref() {
            Some(ctx) if crate::brain::hypr_alive(ctx) => {
                let w = &ctx.window;
                if w.address.is_empty() {
                    (None, None, None, false)
                } else {
                    (
                        Some(w.address.clone()),
                        Some(w.title.clone()),
                        Some(w.class.clone()),
                        w.is_fullscreen,
                    )
                }
            }
            _ => match hypr::active_window_info() {
                Some((a, t, fs)) => {
                    let class = hypr::active_window_where().map(|(c, _)| c);
                    (Some(a), Some(t), class, fs)
                }
                None => (None, None, None, false),
            },
        };
        if self.options_active_addr != addr {
            // Focus moved — re-derive the copy-link affordance from the new app's
            // class (only browsers expose a copyable page URL).
            let is_browser = class.is_some_and(|c| hypr::is_browser_class(&c));
            self.set_clip_link_available(is_browser);
        }
        if self.options_active_addr != addr || self.options_title != title {
            self.options_active_addr = addr;
            self.options_title = title;
            self.measure_options_text();
            self.sync_options_input();
            self.draw_options();
        }
        self.set_options_fullscreen(fullscreen);
    }

    /// One tick of the live-resize watcher; returns the delay until the next
    /// tick. The readout shows while the pointer sits in the focused
    /// window's border grab band (the moment the cursor turns into a resize
    /// arrow — showing the CURRENT size before anything moves) and while the
    /// SAME window's size is changing (the drag itself, a keyboard resize —
    /// a focus switch changes address and never triggers). It reverts to
    /// the title once the pointer is off the border and the size has held
    /// still for [`SIZE_HOLD`]. Sampling is slow at rest, fast while the
    /// readout is up so the numbers track the drag.
    pub(crate) fn tick_resize_watch(&mut self) -> Duration {
        let cur = hypr::active_window_box();
        // A size change of the SAME window marks a resize in progress.
        if let (Some((pa, _, _, pw, ph)), Some((a, _, _, w, h))) =
            (&self.options_size_seen, &cur)
        {
            if pa == a && (pw != w || ph != h) {
                self.options_resize_at = Some(Instant::now());
            }
        }
        // Pointer within the border grab band = the resize cursor is up.
        // (Not meaningful on a fullscreen window — its edges hug the
        // screen's, and there is nothing to resize.)
        let on_border = !self.options_fullscreen
            && match (&cur, hypr::cursor_pos()) {
                (Some((_, x, y, w, h)), Some((px, py))) => {
                    let near = px >= x - BORDER_BAND
                        && px <= x + w + BORDER_BAND
                        && py >= y - BORDER_BAND
                        && py <= y + h + BORDER_BAND;
                    let inside = px >= x + BORDER_BAND
                        && px <= x + w - BORDER_BAND
                        && py >= y + BORDER_BAND
                        && py <= y + h - BORDER_BAND;
                    near && !inside
                }
                _ => false,
            };
        let resizing = self
            .options_resize_at
            .is_some_and(|t| t.elapsed() < SIZE_HOLD);
        let live = ((on_border || resizing) && cur.is_some())
            .then(|| cur.as_ref().map(|(_, _, _, w, h)| (*w, *h)))
            .flatten();
        self.options_size_seen = cur;
        if live != self.options_resize_live {
            self.options_resize_live = live;
            if live.is_none() {
                self.options_resize_at = None;
            }
            self.measure_options_text();
            self.draw_options();
        }
        if self.options_resize_live.is_some() {
            SIZE_POLL_FAST
        } else {
            SIZE_POLL_SLOW
        }
    }

    /// React to the focused window entering/leaving fullscreen: conceal the
    /// bar while fullscreen (it reveals on a deliberate top-edge hold), show it
    /// again otherwise.
    fn set_options_fullscreen(&mut self, fs: bool) {
        if fs == self.options_fullscreen {
            return;
        }
        self.options_fullscreen = fs;
        self.options_reveal_deadline = None;
        self.options_hide_deadline = None;
        // While the overview is open the bar always shows (it has its own
        // reserved strip there) — fullscreen conceal resumes after.
        self.options_hidden = fs && !self.overview_active;
        if fs {
            self.options_hover = None;
        }
        self.sync_options_input();
        self.draw_options();
    }

    /// Arm the dwell timer that reveals a concealed bar. Idempotent while pending.
    fn arm_options_reveal(&mut self) {
        if self.options_reveal_deadline.is_some() {
            return;
        }
        let deadline = Instant::now() + REVEAL_DWELL;
        self.options_reveal_deadline = Some(deadline);
        let timer = Timer::from_duration(REVEAL_DWELL);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.options_reveal_deadline == Some(deadline) {
                    app.options_reveal_deadline = None;
                    let still_at_top = app.options_ptr.is_some_and(|(_, y)| y <= REVEAL_PX);
                    if app.options_hidden && still_at_top {
                        app.options_hidden = false;
                        app.sync_options_input();
                        app.draw_options();
                    }
                }
                TimeoutAction::Drop
            });
    }

    /// Conceal the revealed bar after the grace period (unless the pointer came
    /// back or fullscreen ended).
    fn schedule_options_hide(&mut self) {
        let deadline = Instant::now() + HIDE_GRACE;
        self.options_hide_deadline = Some(deadline);
        let timer = Timer::from_duration(HIDE_GRACE);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.options_hide_deadline == Some(deadline) {
                    app.options_hide_deadline = None;
                    if app.options_fullscreen && !app.options_hidden && app.options_ptr.is_none() {
                        app.options_hidden = true;
                        app.options_hover = None;
                        app.sync_options_input();
                        app.draw_options();
                    }
                }
                TimeoutAction::Drop
            });
    }

    /// Tick the clock (and refresh the date, which changes at midnight);
    /// returns whether anything displayed changed.
    pub(crate) fn tick_options_clock(&mut self) -> bool {
        let clock = clock_now();
        let date = date_now();
        let changed = clock != self.options_clock || date != self.options_date;
        if changed {
            self.options_clock = clock;
            self.options_date = date;
            self.measure_options_text();
        }
        changed
    }

    /// Set the surface's pointer input region: the whole bar strip while shown
    /// (so hover works across pills and the reveal can auto-hide on leave), or
    /// just the thin top reveal strip while concealed in fullscreen.
    pub(crate) fn sync_options_input(&mut self) {
        let (w, _) = self.options_size;
        if w == 0 {
            return;
        }
        let Some(layer) = self.options_layer.as_ref() else {
            return;
        };
        let h = if self.options_hidden {
            REVEAL_PX.ceil() as i32
        } else if self.notif.expanded || self.clip.expanded {
            // Extend the pointer-sensitive region down over whichever history
            // box is open so scroll/hover there reach us instead of passing
            // through. Use the *fully-expanded* bottom (not the live animating
            // height) so the region is stable the instant a box opens.
            let mut bottom = self.config.options.height as f32;
            if self.notif.expanded {
                bottom = bottom.max(self.notif_input_bottom());
            }
            if self.clip.expanded {
                bottom = bottom.max(self.clip_input_bottom());
            }
            bottom.ceil() as i32
        } else {
            self.config.options.height as i32
        };
        surface::set_input_rects(&self.compositor, layer, &[(0, 0, w as i32, h)]);
    }

    /// Classify which surface a pointer `Enter` targets.
    pub(crate) fn classify_pointer_surface(
        &self,
        surface: &wayland_client::protocol::wl_surface::WlSurface,
    ) -> PointerSurface {
        if self
            .options_layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            PointerSurface::Options
        } else {
            PointerSurface::Dock
        }
    }

    /// Route a pointer event that belongs to the OPTIONS surface.
    pub(crate) fn options_pointer(&mut self, event: wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                self.enter_serial = serial;
                self.cursor_now = None;
                self.options_ptr = Some((surface_x as f32, surface_y as f32));
                self.options_on_motion(surface_y as f32);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                self.options_ptr = Some((surface_x as f32, surface_y as f32));
                self.options_on_motion(surface_y as f32);
            }
            wl_pointer::Event::Leave { .. } => {
                self.options_ptr = None;
                self.pointer_surface = PointerSurface::Dock;
                self.options_reveal_deadline = None;
                if !self.options_hidden {
                    self.options_hover = None;
                    self.update_ctrl_reveal(); // fade the buttons out
                    self.update_clock_meta(); // start the date's hold-then-collapse
                    self.update_notif_reveal(); // collapse the bell's peek/history
                    self.update_clip_reveal(); // collapse the clipboard's peek
                    self.update_notif_hit(); // drop any card/control hover (ptr gone)
                    self.update_clip_hit(); // drop any clip row hover (ptr gone)
                    self.draw_options();
                    // Revealed in fullscreen: conceal again shortly after leave.
                    if self.options_fullscreen {
                        self.schedule_options_hide();
                    }
                }
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_LEFT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released)
                    && !self.options_hidden =>
            {
                self.options_click();
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_RIGHT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released)
                    && !self.options_hidden =>
            {
                self.options_right_click();
            }
            // Scroll over the notification OPTION: browse history / expand the
            // list. Works over the bell *or* the mute pill it reveals (both are
            // the one OPTION), or whenever its history is already open.
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } if !self.options_hidden
                && (matches!(self.options_hover, Some(PillId::Notif | PillId::NotifMute))
                    || self.notif.expanded) =>
            {
                self.notif_axis(value as f32);
            }
            // Scroll over the clipboard OPTION: open / browse the clip history.
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } if !self.options_hidden
                && (matches!(self.options_hover, Some(PillId::Clipboard | PillId::ClipboardBox))
                    || self.clip.expanded) =>
            {
                self.clip_axis(value as f32);
            }
            _ => {}
        }
    }

    /// Shared Enter/Motion logic: reveal-dwell at the top edge while concealed,
    /// otherwise hover the pills and cancel any pending conceal.
    fn options_on_motion(&mut self, y: f32) {
        if self.options_hidden {
            if y <= REVEAL_PX {
                self.arm_options_reveal();
            } else {
                self.options_reveal_deadline = None;
            }
        } else {
            self.options_hide_deadline = None;
            self.options_update_hover();
        }
        self.options_apply_cursor();
    }

    fn options_update_hover(&mut self) {
        let bar_h = self.config.options.height as f32;
        let hover = self.options_ptr.and_then(|p| {
            self.options_pills()
                .iter()
                .find(|pill| {
                    // The notification element owns its whole (possibly tall,
                    // below-the-bar) rect so hover holds while its history is
                    // open; every other pill gets the full-bar-height hit (up to
                    // the top screen edge) so slamming to the edge still lands.
                    let hit = if matches!(pill.id, PillId::Notif | PillId::ClipboardBox) {
                        Rect::new(pill.rect.x, 0.0, pill.rect.w, pill.rect.y + pill.rect.h)
                    } else {
                        Rect::new(pill.rect.x, 0.0, pill.rect.w, bar_h)
                    };
                    hit.contains(p) && self.ctrl_pill_visible(pill.id)
                })
                .map(|pill| pill.id)
        });
        let changed = hover != self.options_hover;
        self.options_hover = hover;
        self.update_ctrl_reveal();
        self.update_clock_meta();
        self.update_notif_reveal();
        self.update_clip_reveal();
        // The hit target (card / control / footer) moves within the same box, so
        // redraw on a hit change too — not just when the pill changes.
        let hit_changed = self.update_notif_hit();
        let clip_hit_changed = self.update_clip_hit();
        if changed || hit_changed || clip_hit_changed {
            self.draw_options();
        }
    }

    /// A control button is only hoverable/clickable once mostly revealed.
    fn ctrl_pill_visible(&self, id: PillId) -> bool {
        // The mute pill hides behind the bell at rest; only accept hover/clicks
        // once it has actually been uncovered, so the resting bell slot always
        // hits the bell (which is listed first) rather than the pill under it.
        if id == PillId::NotifMute {
            return self.notif.peek_progress() > 0.5;
        }
        // The copy-link pill hides behind the clipboard pill at rest; only accept
        // hits once it's mostly slid out (so the resting slot hits the clipboard
        // pill, listed after it).
        if id == PillId::ClipCopyLink {
            return self.clip_link_t() > 0.5;
        }
        match ctrl_index(id) {
            Some(i) => self.options_ctrl.t[i] > 0.5,
            None => true, // window / clock / close always
        }
    }

    /// Whether the pointer is within the window+controls cluster span (so a
    /// small gap between pills doesn't count as leaving).
    fn options_ptr_in_cluster(&self) -> bool {
        let Some((x, y)) = self.options_ptr else {
            return false;
        };
        if y < 0.0 || y > self.config.options.height as f32 {
            return false;
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &self.options_pills() {
            if p.id == PillId::Window || p.id == PillId::Close || ctrl_index(p.id).is_some() {
                lo = lo.min(p.rect.x);
                hi = hi.max(p.rect.x + p.rect.w);
            }
        }
        x >= lo && x <= hi
    }

    /// Update whether the mode toggles should be revealed: they appear when
    /// the window pill or the (always-visible) close button is hovered and
    /// stay while the pointer is over the cluster; leaving fades them out. A
    /// fresh reveal restarts the slide.
    fn update_ctrl_reveal(&mut self) {
        let want = if self.options_ctrl.reveal {
            self.options_ptr_in_cluster()
        } else {
            matches!(
                self.options_hover,
                Some(PillId::Window) | Some(PillId::Close)
            )
        };
        if want != self.options_ctrl.reveal {
            self.options_ctrl.reveal = want;
            // Progress is NOT reset: a flip mid-flight continues from where
            // each button is, so quick hover in/out reverses smoothly.
            self.options_ctrl.changed_at = Some(Instant::now());
            self.options_ctrl.last = None;
            self.schedule_options_ctrl_frame();
        }
    }

    fn schedule_options_ctrl_frame(&mut self) {
        if self.options_ctrl.frame_pending {
            return;
        }
        self.options_ctrl.frame_pending = true;
        if self.options_ctrl.last.is_none() {
            self.options_ctrl.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_ctrl.frame_pending = false;
                app.tick_options_ctrl();
                TimeoutAction::Drop
            });
    }

    /// Advance the control-button reveal one frame and keep frames coming until
    /// everything settles.
    fn tick_options_ctrl(&mut self) {
        let now = Instant::now();
        let dt = self
            .options_ctrl
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.options_ctrl.last = Some(now);
        let elapsed = self
            .options_ctrl
            .changed_at
            .map_or(f32::MAX, |t| now.duration_since(t).as_secs_f32());
        let reveal = self.options_ctrl.reveal;
        let mut active = false;
        for i in 0..CTRL_N {
            // The reveal chain emerges inner-first; the hide retracts
            // outermost-first — each button waits its stagger turn and holds
            // where it is until then.
            let order = if reveal { i } else { CTRL_N - 1 - i };
            let due = elapsed >= order as f32 * CTRL_STAGGER;
            if !due {
                active = true; // its turn is coming — keep ticking
                continue;
            }
            let target = if reveal { 1.0 } else { 0.0 };
            let (nt, moving) =
                ease_toward(self.options_ctrl.t[i], target, dt, CTRL_RATE, CTRL_EPS);
            self.options_ctrl.t[i] = nt;
            active |= moving;
        }
        self.draw_options();
        if active {
            self.schedule_options_ctrl_frame();
        } else {
            self.options_ctrl.last = None;
        }
    }

    /// Update whether the clock pill should show the date: revealed while it's
    /// hovered; on leave it holds on the date for [`META_HOLD`] then collapses
    /// back to the clock (same transition backwards).
    fn update_clock_meta(&mut self) {
        if self.options_hover == Some(PillId::Clock) {
            // Hovering the clock: reveal, and cancel any pending collapse.
            self.options_clock_meta.hold_deadline = None;
            if !self.options_clock_meta.reveal {
                self.options_clock_meta.reveal = true;
                self.options_clock_meta.last = None;
                self.schedule_options_clock_frame();
            }
        } else if self.options_clock_meta.reveal && self.options_clock_meta.hold_deadline.is_none()
        {
            // Left the clock while showing the date: hold, then collapse.
            self.schedule_clock_collapse();
        }
    }

    /// After the hover leaves, keep the date up for [`META_HOLD`], then play
    /// the metamorphosis backwards (unless the clock got hovered again).
    fn schedule_clock_collapse(&mut self) {
        let deadline = Instant::now() + META_HOLD;
        self.options_clock_meta.hold_deadline = Some(deadline);
        let timer = Timer::from_duration(META_HOLD);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.options_clock_meta.hold_deadline == Some(deadline) {
                    app.options_clock_meta.hold_deadline = None;
                    if app.options_hover != Some(PillId::Clock) {
                        app.options_clock_meta.reveal = false;
                        app.options_clock_meta.last = None;
                        app.schedule_options_clock_frame();
                    }
                }
                TimeoutAction::Drop
            });
    }

    fn schedule_options_clock_frame(&mut self) {
        if self.options_clock_meta.frame_pending {
            return;
        }
        self.options_clock_meta.frame_pending = true;
        if self.options_clock_meta.last.is_none() {
            self.options_clock_meta.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_clock_meta.frame_pending = false;
                app.tick_options_clock_meta();
                TimeoutAction::Drop
            });
    }

    /// Advance the clock↔date metamorphosis one frame; the pill width and the
    /// crossfade are both derived from `t` at draw time.
    fn tick_options_clock_meta(&mut self) {
        let now = Instant::now();
        let dt = self
            .options_clock_meta
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.options_clock_meta.last = Some(now);
        let target = if self.options_clock_meta.reveal {
            1.0
        } else {
            0.0
        };
        let (nt, moving) = ease_toward(self.options_clock_meta.t, target, dt, META_RATE, META_EPS);
        self.options_clock_meta.t = nt;
        self.draw_options();
        if moving {
            self.schedule_options_clock_frame();
        } else {
            self.options_clock_meta.last = None;
        }
    }

    fn options_click(&mut self) {
        // The open notification box handles its own hits (cards / controls /
        // footer / menu) first; if it consumes the click, stop.
        if self.notif_click() {
            return;
        }
        // The open clipboard box handles its own hits (row = copy, × = delete,
        // clear-all); consume the click so it doesn't fall through to paste.
        if self.clip.expanded && self.options_hover == Some(PillId::ClipboardBox) {
            self.clip_box_click();
            return;
        }
        match self.options_hover {
            Some(PillId::Close) => {
                if let Some(addr) = self.options_active_addr.clone() {
                    hypr::close_window(&addr);
                }
            }
            Some(PillId::Pseudo) => hypr::pseudo_active(),
            Some(PillId::Fullscreen) => hypr::fullscreen_active(),
            Some(PillId::NotifMute) => self.toggle_notif_mute(),
            // Clicking the clipboard element pastes the current clip into the
            // focused window. Both ids resolve here because the box overlaps the
            // small pill at rest (a scrollable history box will split these in a
            // later stage).
            Some(PillId::Clipboard | PillId::ClipboardBox) => self.clip_paste(),
            // Copy the focused browser's current page URL to the clipboard.
            Some(PillId::ClipCopyLink) => self.copy_active_link(),
            _ => {}
        }
    }

    /// Right-click: over an open clipboard row, open its metadata detail view.
    fn options_right_click(&mut self) {
        if self.clip.expanded && self.options_hover == Some(PillId::ClipboardBox) {
            self.clip_box_right_click();
        }
    }

    fn options_apply_cursor(&mut self) {
        let Some(device) = &self.cursor_device else {
            return;
        };
        let shape = match self.options_hover {
            // Over the notification element: a pointer on a clickable target inside
            // the open box (an openable card / footer button), else default.
            Some(PillId::Notif) => {
                if self.notif_hit_clickable() {
                    Shape::Pointer
                } else {
                    Shape::Default
                }
            }
            // The open clipboard box: pointer only on a clickable target (a row /
            // delete / clear-all), default over the empty fill.
            Some(PillId::ClipboardBox) => {
                if self.clip_box_hit_clickable() {
                    Shape::Pointer
                } else {
                    Shape::Default
                }
            }
            // The small clipboard pill is clickable (paste) → pointer.
            Some(PillId::Clock | PillId::Window) | None => Shape::Default,
            Some(_) => Shape::Pointer, // control circle / small clipboard pill
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }
}

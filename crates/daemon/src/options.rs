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
use tracing::{info, warn};
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

// Dynamic OPTION-pill glyphs (the Mind's context-aware controls). Keyed by
// affordance id in `glyph_for_option`.
const GLYPH_PLAY: &str = "\u{f04b}"; // fa-play
const GLYPH_PAUSE: &str = "\u{f04c}"; // fa-pause
const GLYPH_VOL_DOWN: &str = "\u{f027}"; // fa-volume-down
const GLYPH_VOL_UP: &str = "\u{f028}"; // fa-volume-up
const GLYPH_VOL_MUTE: &str = "\u{f026}"; // fa-volume-off (mute)
const GLYPH_BRIGHT_UP: &str = "\u{f185}"; // fa-sun-o
const GLYPH_BRIGHT_DOWN: &str = "\u{f042}"; // fa-adjust (dim)
const GLYPH_NEXT: &str = "\u{f051}"; // fa-step-forward
const GLYPH_PREV: &str = "\u{f048}"; // fa-step-backward
const GLYPH_SEEK_FWD: &str = "\u{f04e}"; // fa-forward (seek +10s)
const GLYPH_SEEK_BACK: &str = "\u{f04a}"; // fa-backward (seek -10s)
const GLYPH_MIC_SLASH: &str = "\u{f131}"; // fa-microphone-slash (mute mic)
const GLYPH_COMMIT: &str = "\u{f00c}"; // fa-check (commit)
const GLYPH_PUSH: &str = "\u{f093}"; // fa-upload (push)
const GLYPH_PULL: &str = "\u{f019}"; // fa-download (pull)
const GLYPH_REMOTE: &str = "\u{f09b}"; // fa-github (open remote)
const GLYPH_DIFF: &str = "\u{f440}"; // cod-diff (show diff)
const GLYPH_SEARCH: &str = "\u{f002}"; // fa-search (search the web)
const GLYPH_OPEN_FILE: &str = "\u{f07c}"; // fa-folder-open (open copied path)
const GLYPH_EMAIL: &str = "\u{f0e0}"; // fa-envelope (compose email)
const GLYPH_MONITOR: &str = "\u{f0e4}"; // fa-tachometer (system monitor)
const GLYPH_TERMINAL: &str = "\u{f120}"; // fa-terminal (open terminal here)
const GLYPH_RERUN: &str = "\u{f021}"; // fa-refresh (re-run last command)
const GLYPH_FORMAT: &str = "\u{f0d0}"; // fa-magic (format the file)
const GLYPH_WIFI: &str = "\u{f1eb}"; // fa-wifi (network state / settings)
const GLYPH_CAMERA: &str = "\u{f030}"; // fa-camera (camera live)
const GLYPH_MIC: &str = "\u{f130}"; // fa-microphone (mic live)
const GLYPH_SCREENCAST: &str = "\u{f108}"; // fa-desktop (screen sharing)
const GLYPH_OPTION: &str = "\u{f0eb}"; // fa-lightbulb-o (generic OPTION)
const GLYPH_MUSIC: &str = "\u{f001}"; // fa-music (open the media box)
/// Amber wash for a privacy/safety WARNING pill, so it reads as "heads up",
/// not a button.
const WARN_COLOR: [f32; 4] = [1.0, 0.72, 0.30, 1.0];

/// Whether an affordance is surfaced as an OPTION pill: any actionable control,
/// plus the privacy/safety WARNINGS worth a persistent glance (a live camera,
/// mic, or screen share). Battery/deploy warnings are excluded — they have
/// their own dedicated surfaces (battery.rs, and the deploy nudge).
pub(crate) fn is_surfaced_affordance(a: &options_engine::Affordance) -> bool {
    a.action.is_actionable()
        || (a.kind == options_engine::AffordanceKind::Warning
            && matches!(
                a.id,
                "camera.live" | "audio.mic_live" | "compositor.screencasting" | "network.down"
            ))
}

/// The Nerd-Font glyph for a dynamic OPTION control, by affordance id. The
/// play/pause toggle reads its title so it shows the action it WILL perform.
fn glyph_for_option(id: &str, title: &str) -> &'static str {
    match id {
        "media.playpause" => {
            if title == "Pause" {
                GLYPH_PAUSE
            } else {
                GLYPH_PLAY
            }
        }
        "media.vol_down" => GLYPH_VOL_DOWN,
        "media.vol_up" => GLYPH_VOL_UP,
        "media.mute" => GLYPH_VOL_MUTE,
        "media.bright_up" | "reading.bright_up" => GLYPH_BRIGHT_UP,
        "media.bright_down" | "reading.bright_down" => GLYPH_BRIGHT_DOWN,
        "media.next" | "slides.next" | "reading.page_next" => GLYPH_NEXT,
        "media.prev" | "slides.prev" | "reading.page_prev" => GLYPH_PREV,
        "media.seek_fwd" => GLYPH_SEEK_FWD,
        "media.seek_back" => GLYPH_SEEK_BACK,
        "audio.mic_mute" => GLYPH_MIC_SLASH,
        "audio.call_dnd" | "window.fullscreen_dnd" => GLYPH_BELL_SLASH,
        "window.screenshot" => GLYPH_CAMERA,
        "camera.live" => GLYPH_CAMERA,
        "audio.mic_live" => GLYPH_MIC,
        "compositor.screencasting" => GLYPH_SCREENCAST,
        "git.commit" => GLYPH_COMMIT,
        "git.push" => GLYPH_PUSH,
        "git.pull" => GLYPH_PULL,
        "git.open_remote" => GLYPH_REMOTE,
        "git.diff" | "git.show_commit" => GLYPH_DIFF,
        "selection.url" => GLYPH_COPY_LINK,
        "selection.open_path" => GLYPH_OPEN_FILE,
        "selection.email" => GLYPH_EMAIL,
        "selection.search" | "shell.search_error" | "browser.find" | "reading.find" => GLYPH_SEARCH,
        "browser.reopen_tab" => GLYPH_RERUN,
        "system.high_cpu" | "system.high_mem" => GLYPH_MONITOR,
        "system.battery_dim" => GLYPH_BRIGHT_DOWN,
        "downloads.open" | "shell.install_missing" => GLYPH_PULL,
        "downloads.extract" => GLYPH_OPEN_FILE,
        "coding.terminal_here" => GLYPH_TERMINAL,
        "shell.rerun" => GLYPH_RERUN,
        "editor.run" | "slides.present" => GLYPH_PLAY,
        "editor.build" => GLYPH_TERMINAL,
        "editor.format" => GLYPH_FORMAT,
        "network.down" | "network.settings" => GLYPH_WIFI,
        "files.open_here" | "editor.open_folder" => GLYPH_OPEN_FILE,
        _ => GLYPH_OPTION,
    }
}

/// How many dynamic OPTION pills the topbar shows at once — a de-cluttered
/// cluster that stays clear of the centred window pill.
const OPTION_PILL_CAP: usize = 5;

/// The `/bin/sh -c` command line for a spawn-style [`options_engine::
/// AffordanceAction`], or `None` when the action isn't a spawn (or is empty).
/// Each argv element is shell-quoted, so a path or URL carrying spaces or shell
/// metacharacters is passed literally — no injection surface even though the
/// action ultimately runs through a shell. Pure, so the quoting is unit-tested.
fn action_command_line(action: &options_engine::AffordanceAction) -> Option<String> {
    use options_engine::AffordanceAction as A;
    match action {
        A::Spawn { argv } if !argv.is_empty() => Some(
            argv.iter()
                .map(|a| crate::launch::shell_quote(a))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        A::OpenUrl(url) => Some(format!("xdg-open {}", crate::launch::shell_quote(url))),
        _ => None,
    }
}

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
// While a resize DRAG is in flight (click on a border / Super+RMB — waveview
// watches the compositor's drag state and writes resize-drag-on/off to the
// control socket), the window pill appends the focused window's live size:
// `current task (1992x1199)`. Event-driven: nothing polls at rest; the
// readout appears at the click, before anything moves (Max, 2026-08-31 —
// the earlier hover-band trigger fired involuntarily near the topbar).
/// Sampling interval while the readout is up — fast enough to read as a
/// continuous counter tracking the drag.
const SIZE_POLL_FAST: Duration = Duration::from_millis(40);

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
        // Dynamic OPTION controls sit in the free left-centre band, overlapping
        // nothing — drawn first (lowest z).
        PillId::Option(_) => 0,
        PillId::MediaOpen => 0,
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
    /// A dynamic OPTION control from the Mind (media/git/call/…), identified by
    /// its index into [`crate::App::surfaced_options`]. Clicking it runs that
    /// affordance's action.
    Option(u8),
    /// The media-box opener: a music glyph shown when a player is active; a
    /// click grows the transport box (see [`crate::mediabox`]).
    MediaOpen,
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

/// The situations an OPTIONS element belongs in.
///
/// This is the bar's **presence contract**: every element declares where it
/// makes sense, and [`App::options_pills`] filters the list once — the one
/// place the bar's composition is decided, and the only input to drawing,
/// hit-testing, hover, and clicks. An element that isn't present cannot be
/// seen or touched, so nothing downstream needs a situation check.
///
/// It replaces scattered `if overview_active` special-casing (Max,
/// 2026-08-31: "i dont want to start patching OPTIONS"), and it is the seam
/// the Brain plugs into: today presence is a static answer, later the same
/// call site asks `options_engine`'s mind for relevance instead — without
/// any call site moving.
#[derive(Clone, Copy)]
struct Presence {
    /// The normal session.
    desktop: bool,
    /// While the waveview overview owns the screen.
    overview: bool,
}

/// Present everywhere.
const BOTH: Presence = Presence {
    desktop: true,
    overview: true,
};
/// Only over the live desktop.
const DESKTOP_ONLY: Presence = Presence {
    desktop: true,
    overview: false,
};

/// The theme's own box colour — the neutral OPTIONS slab, used when nothing
/// has been sampled yet.
const BOX_SLAB: [f32; 3] = [0.10, 0.10, 0.12];
/// How opaque an open box's PANEL (and its zebra bands) are. Below 1.0 so
/// the compositor's blur reads through them as frosted glass; high enough
/// that the fill's own tint still governs the ink choice.
///
/// The zebra must carry the same value: a stripe drawn opaque over a
/// translucent panel hides the blur under every other row, which is exactly
/// how it looked — one band frosted, the next flat.
pub(crate) const BOX_ALPHA: f32 = 0.80;

impl crate::App {
    /// The open boxes' panel/zebra alpha: [`BOX_ALPHA`] glass normally,
    /// fully opaque under the reduce-transparency intent.
    pub(crate) fn box_panel_alpha(&self) -> f32 {
        if self.config.accessibility.reduce_transparency {
            1.0
        } else {
            BOX_ALPHA
        }
    }
}
/// An open box's fill: the backdrop it floats on with the pill's wash
/// composited over it, so the box reads as **the pill grown** and sits close
/// to the surrounding colour (Max, 2026-08-31: "we want a similar color to
/// the bg color").
///
/// No darkening: forcing the sample to a target luminance made the boxes
/// heavy slabs that no longer belonged to the wallpaper. It is safe to stay
/// this close now because the INK measures each surface (see [`ink_on`]) —
/// a light box simply takes dark text, exactly as the bar does over the same
/// light wallpaper. Chroma survives because the wash is weak.
fn box_fill(backdrop: [f32; 4], wash: [f32; 4]) -> [f32; 4] {
    let a = wash[3];
    [
        backdrop[0] * (1.0 - a) + wash[0] * a,
        backdrop[1] * (1.0 - a) + wash[1] * a,
        backdrop[2] * (1.0 - a) + wash[2] * a,
        1.0,
    ]
}

// OPTIONS' ink: never pure black or white, and always a touch WARM (r > g >
// b) so the text sits in the room's light instead of glaring out of it (Max,
// 2026-08-31). LINEAR values — the swapchain encodes sRGB on write — so the
// sRGB the eye gets is in each comment.
/// Warm off-white ≈ #E8E5DE.
const INK_LIGHT: [f32; 4] = [0.807, 0.787, 0.728, 1.0];
/// Warm near-black ≈ #262220.
const INK_DARK: [f32; 4] = [0.019, 0.016, 0.014, 1.0];

/// Relative luminance of a LINEAR colour (the space every colour here lives
/// in — the swapchain encodes sRGB on write).
fn luminance(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Rounded frame around the hovered row/card, in the box's own ink at
/// [`HOVER_FRAME_ALPHA`].
///
/// One STROKED instance ([`RectInst::border`]), so the outline follows its
/// rounded corners exactly — four thin rects can only ever draw a square
/// one, and the outer-plus-inner trick would need an opaque inner fill that
/// would punch a hole in the box's frosted blur.
pub(crate) fn push_hover_frame(scene: &mut Scene, r: Rect, ink: [f32; 4], alpha: f32) {
    /// Line thickness and corner radius, logical px.
    const W: f32 = 1.0;
    const RADIUS: f32 = 7.0;
    // No inset: the frame sits ON the card's own edge (the stroke occupies
    // the outermost pixel), so it traces the card rather than floating
    // inside it.
    if r.w <= 2.0 * W || r.h <= 2.0 * W {
        return;
    }
    scene.rects.push(RectInst {
        rect: r,
        radius: RADIUS,
        color: [ink[0], ink[1], ink[2], alpha],
        glass: 0.0,
        border: W,
    });
}

/// How present the hovered row's frame is. Restrained, but it has to read at
/// a glance: a hairline only marks the row if you can actually see it.
pub(crate) const HOVER_FRAME_ALPHA: f32 = 0.55;

/// The hovered line's ink: the SAME colour at full strength — the hover
/// always moves the text AWAY from its background, never toward it.
///
/// Two attempts failed before this one, both for the same reason. "Lighter
/// on hover" reads as emphasis only on a dark box; on a light one it drops
/// the text from 5.5:1 to 2.5:1, so the hovered row looks *faded* rather
/// than picked (Max, 2026-08-31: "in black text there is no hover"). And
/// with the resting text already at full strength there was no headroom
/// left in the direction that does read.
///
/// So the resting list sits a little under full (see each box's `LIST_DIM*`)
/// and hover spends that headroom: dark ink deepens toward black, light ink
/// brightens toward white. One rule, correct in both regimes, and no row or
/// card tinting.
pub(crate) fn hover_ink_for(ink: [f32; 4]) -> [f32; 4] {
    // Just full strength — the colour itself doesn't move. Pushing it past
    // the resting ink (toward black/white) was needed back when lightness
    // was carrying the hint alone; with WEIGHT doing that job the extra
    // darkening only made the hover heavy (Max, 2026-09-01: "the bold is too
    // agresive", and the colours now read well as they are).
    [ink[0], ink[1], ink[2], 1.0]
}

/// The ink that reads on `bg`: dark on a bright surface, light on a dark one.
/// 0.179 is the WCAG flip point where black and white contrast equally.
///
/// THE rule for every OPTIONS surface — each asks about the background it
/// actually sits on, and gets an answer that is legible there. The bar's
/// pills sit on the (transparent) bar's backdrop; an open box sits on its own
/// opaque fill. They can therefore differ, and should: that is not the
/// inconsistency Max hit earlier, which was a surface using a *stale, static*
/// answer (the theme's white over a light wallpaper) instead of measuring.
fn ink_on(bg: [f32; 4]) -> [f32; 4] {
    if luminance(bg) > 0.179 {
        INK_DARK
    } else {
        INK_LIGHT
    }
}

/// Where each element belongs. Read it top to bottom to know the bar.
fn presence(id: PillId) -> Presence {
    match id {
        // The overview needs a label for what you're pointing at, its exit
        // (the X closes it), and the clock as furniture.
        PillId::Window | PillId::Close | PillId::Clock => BOTH,
        // A notification arriving while you pick a window is still worth
        // seeing, and the bell doesn't act on the focused window.
        PillId::Notif | PillId::NotifMute => BOTH,
        // Window-mode controls act on the FOCUSED window — meaningless while
        // you're above the desktop choosing one.
        PillId::Pseudo | PillId::Fullscreen => DESKTOP_ONLY,
        // The clipboard serves the window you're working in, not the map.
        PillId::Clipboard | PillId::ClipboardBox | PillId::ClipCopyLink => DESKTOP_ONLY,
        // Context controls act on the focused app — meaningless over the map.
        PillId::Option(_) => DESKTOP_ONLY,
        PillId::MediaOpen => DESKTOP_ONLY,
    }
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

        // Dynamic OPTION pills: the Mind's context-aware controls (media
        // transport/volume/brightness, git commit/push, mute mic, open link).
        // Small glyph circles in the free band just right of the clipboard
        // cluster, in the mind's ranked order. Each carries its index into
        // `surfaced_options()`, which the click handler dispatches.
        {
            // Clear the clipboard pill + the copy-link's slide-out slot.
            let cluster_start = EDGE_PAD + 2.0 * ph + crate::clipboard::LINK_GAP + OPTION_GAP;
            // (index, glyph, warning?) — a privacy/safety warning pill washes
            // amber and is a passive indicator, not a button.
            let glyphs: Vec<(u8, &'static str, bool)> = self
                .surfaced_options()
                .iter()
                .take(OPTION_PILL_CAP)
                .enumerate()
                .map(|(i, a)| {
                    let warn = a.kind == options_engine::AffordanceKind::Warning;
                    (i as u8, glyph_for_option(a.id, &a.title), warn)
                })
                .collect();
            let mut ox = cluster_start;
            for (i, glyph, warn) in glyphs {
                pills.push(Pill {
                    id: PillId::Option(i),
                    rect: Rect::new(ox, y, ph, ph),
                    text: glyph.to_owned(),
                    family: Some(NERD),
                    glyph_color: warn.then_some(WARN_COLOR),
                });
                ox += ph + CTRL_GAP;
            }
            // The media-box opener: a music glyph when a player is active, right
            // of the control cluster. A click grows the transport box.
            if self.media_now().is_some() {
                pills.push(Pill {
                    id: PillId::MediaOpen,
                    rect: Rect::new(ox, y, ph, ph),
                    text: GLYPH_MUSIC.to_owned(),
                    family: Some(NERD),
                    glyph_color: None,
                });
            }
        }

        // The window name pill is centred *alone* (so it doesn't shift when the
        // toggles reveal); close rests beside it, ALWAYS visible; the mode
        // toggles hide until hover, at fixed resting spots right of the close:
        //   [window name] [X] [pseudo] [fullscreen]
        if self.options_title.is_some() || self.overview_hover.is_some() {
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
        // The presence contract, applied once: everything downstream (draw,
        // hit-test, hover, click) reads this list, so an element that does
        // not belong in the current situation simply isn't there.
        pills.retain(|p| {
            let pres = presence(p.id);
            if self.overview_active {
                pres.overview
            } else {
                pres.desktop
            }
        });
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
    /// appended — `current task (342x343)` — while a resize is in flight.
    /// In the overview the pill follows the POINTER instead of the focused
    /// window, so a hovered thumbnail's title supersedes it.
    fn options_window_text(&self) -> Option<String> {
        let title = self
            .overview_hover
            .as_ref()
            .filter(|_| self.overview_active)
            .or(self.options_title.as_ref())
            .map(|t| truncate(t, TITLE_MAX))?;
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
    /// MATCHED COLOUR ONLY, on purpose: this drives the pills' washes and
    /// their neumorph shadows, i.e. how the bar *looks*. Only the INK
    /// measures the frosted wallpaper (see [`Self::options_text_color`]) —
    /// teaching this the frost too would restyle every pill on a light
    /// wallpaper, which is not the contrast problem (Max, 2026-08-31: "the
    /// change i asked you is only on the text color").
    pub(crate) fn options_bar_is_bright(&self) -> bool {
        self.options_bar_matched
            .is_some_and(|c| luminance(c) > 0.179)
    }

    /// What the BAR's own pills sit on: the matched window colour when the bar
    /// is painted, else the blurred wallpaper it floats on (sampled
    /// continuously by [`crate::screencopy`]). `None` only before the first
    /// sample lands.
    ///
    /// Consulting the frost here is the fix for unreadable pills over a light
    /// wallpaper: the bar used to fall back to a STATIC theme ink whenever it
    /// wasn't colour-matched, so it painted white text on whatever happened to
    /// be behind it (Max, 2026-08-31: "the contrast is garbage").
    fn options_backdrop(&self) -> Option<[f32; 4]> {
        self.options_bar_matched.or(self.options_pill_color)
    }

    /// Adaptive text colour for the BAR's pills: measured against whatever
    /// they float on, so the words stay legible over a matched window or a
    /// bare wallpaper alike. Only an unsampled bar falls back to the theme.
    /// This is the ONLY thing the frost sample feeds — the pills' own washes
    /// and shadows keep their matched-only behaviour.
    pub(crate) fn options_text_color(&self) -> [f32; 4] {
        match self.options_backdrop() {
            Some(bg) => ink_on(bg),
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

    /// The open OPTIONS boxes' fill + ink (clipboard, notifications).
    ///
    /// A box is "the pill grown", so it follows the BAR'S REGIME instead of
    /// deciding its ink independently — that divergence was a real bug (Max,
    /// 2026-08-31: the bar's text white while both boxes' text was black):
    ///
    /// - **Colour-matched bar** — the matched window colour with the pill
    ///   wash composited on, and the bar's own adaptive ink. These agree by
    ///   construction, since both derive from the same matched colour.
    /// - **Transparent bar** (the theme is in charge, so the bar paints its
    ///   text from the theme) — the sampled wallpaper frost, darkened to a
    ///   legible box by [`frosted_box_fill`]. Taking that frost raw is what
    ///   broke: over a light wallpaper the box became a pale slab and flipped
    ///   its ink to black while the bar, which never consults the frost,
    ///   stayed white.
    ///
    /// (The slab is dark because Golem's theme is: a light theme would flip
    /// both that constant and the ink together.)
    pub(crate) fn options_box_surface(&self) -> ([f32; 4], [f32; 4]) {
        let wash = self.options_rest_wash();
        // One formula for both regimes: the backdrop (matched window colour,
        // else the sampled wallpaper) with the pill wash over it — the box
        // is the pill grown.
        let mut fill = match self.options_backdrop() {
            Some(backdrop) => box_fill(backdrop, wash),
            // Not sampled yet (the first frames after opening): plain slab.
            None => [BOX_SLAB[0], BOX_SLAB[1], BOX_SLAB[2], 1.0],
        };
        // NOTE: opaque. Translucency belongs to the box PANEL and its zebra
        // only (each box applies [`BOX_ALPHA`] there); this fill is also the
        // clip detail card, the dictionary panel and the notification icon
        // discs, and making it translucent wholesale turned those glassy too
        // (Max, 2026-09-01: "the clipboard big pill became transparent... i
        // only want the boxes to be blured").
        fill[3] = 1.0;
        // Ink measured against the box's OWN fill. Since that fill is now the
        // backdrop plus a weak wash, this lands on the same answer the bar
        // reaches for the same backdrop — the two agree by measurement rather
        // than by one borrowing the other's decision.
        (fill, ink_on(fill))
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
    /// Discoverability: while a context OPTION pill (an icon-only glyph) is
    /// hovered, show its offer title in a small label just below the bar. Only
    /// the dynamic OPTION pills get this — the fixed pills (clock, bell,
    /// clipboard) reveal their own labels. Suppressed while the media box is
    /// open (the pointer is working that surface, not reading tooltips).
    pub(crate) fn push_options_tooltip(&self, scene: &mut Scene) {
        if self.media_box_open {
            return;
        }
        let Some(PillId::Option(i)) = self.options_hover else {
            return;
        };
        let Some(label) = self
            .surfaced_options()
            .get(i as usize)
            .map(|a| a.title.clone())
            .filter(|s| !s.trim().is_empty())
        else {
            return;
        };
        let pills = self.options_pills();
        let Some(pr) = pills
            .iter()
            .find(|p| p.id == PillId::Option(i))
            .map(|p| p.rect)
        else {
            return;
        };
        let bar_h = self.config.options.height as f32;
        let full_w = self.options_size.0 as f32;
        // The Label shapes exactly at render time; only the background needs a
        // size, so a per-char estimate is fine here.
        let pad_x = 11.0;
        let text_w = label.chars().count() as f32 * (FONT_PX * 0.62);
        let box_w = (text_w + 2.0 * pad_x).min((full_w - 8.0).max(24.0));
        let box_h = LINE_PX + 9.0;
        let bx = (pr.x + (pr.w - box_w) / 2.0).clamp(4.0, (full_w - box_w - 4.0).max(4.0));
        let by = bar_h + 6.0;
        let (fill, ink) = self.options_box_surface();
        scene.rects.push(RectInst {
            rect: Rect::new(bx, by, box_w, box_h),
            radius: box_h / 2.0,
            color: fill,
            glass: 0.0,
            border: 0.0,
        });
        scene.labels.push(Label {
            text: label,
            pos: (bx + box_w / 2.0, by + (box_h - LINE_PX) / 2.0),
            max_w: box_w - 2.0 * pad_x,
            font_px: FONT_PX,
            line_px: LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            family: None,
            color: Some(ink),
            clip: None,
        });
    }

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
                border: 0.0,
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
        // the overview, so "Overview" is the label a pointer resting on no
        // thumbnail falls back to (a hovered one supersedes it), and there is
        // no window address to act on. Which CONTROLS survive up here is not
        // decided here — see `presence`.
        if self.overview_active {
            let title = Some(crate::i18n::tr("Overview").to_owned());
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
            // Feed the usage-aware focus cycle (walk-driven hops excluded).
            if let Some(a) = addr.clone() {
                self.note_focus_change(&a);
            }
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

    /// One tick of the live-resize watcher: sample the focused window's
    /// size and update the pill; returns the next delay while the drag is
    /// still in flight, `None` when done (the readout tucks away and the
    /// mini-loop drops).
    fn tick_resize_watch(&mut self) -> Option<Duration> {
        let live = self
            .resize_drag
            .then(|| hypr::active_window_geom().map(|(_, _, w, h)| (w, h)))
            .flatten();
        if live != self.options_resize_live {
            self.options_resize_live = live;
            self.measure_options_text();
            self.draw_options();
        }
        if self.resize_drag {
            Some(SIZE_POLL_FAST)
        } else {
            self.resize_watch_running = false;
            None
        }
    }

    /// Start (or let run) the fast sampling mini-loop behind the live
    /// readout. Called when waveview reports a resize drag beginning or
    /// ending: the first tick runs immediately (the size shows at the CLICK,
    /// before anything moves), then the loop ticks fast until the drag ends
    /// and drops itself — nothing runs at rest.
    pub(crate) fn kick_resize_watch(&mut self) {
        if self.resize_watch_running {
            return; // the live loop will pick the state change up itself
        }
        let Some(delay) = self.tick_resize_watch() else {
            return; // drag already over (or no window): readout cleared
        };
        self.resize_watch_running = true;
        let armed =
            self.loop_handle
                .insert_source(Timer::from_duration(delay), |_, _, app: &mut App| match app
                    .tick_resize_watch()
                {
                    Some(d) => TimeoutAction::ToDuration(d),
                    None => TimeoutAction::Drop,
                });
        if let Err(e) = armed {
            self.resize_watch_running = false;
            warn!("resize-watch timer failed ({e}); no live size readout");
        }
    }

    /// The overview's hovered thumbnail changed: the pill follows the
    /// pointer while the overview owns the screen.
    pub(crate) fn set_overview_hover(&mut self, title: Option<String>) {
        if self.overview_hover == title {
            return;
        }
        self.overview_hover = title;
        self.measure_options_text();
        self.draw_options();
    }

    /// React to the focused window entering/leaving fullscreen: conceal the
    /// bar while fullscreen (it reveals on a deliberate top-edge hold), show it
    /// again otherwise.
    fn set_options_fullscreen(&mut self, fs: bool) {
        let changed = fs != self.options_fullscreen;
        if changed {
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
        }
        // Reconcile the screencopy colour-match against fullscreen (pause while
        // a fullscreen client is up, resume on exit). Idempotent, so it also
        // self-heals if a transition is missed.
        self.reconcile_options_fullscreen();
        if changed {
            self.draw_options();
        }
    }

    /// Reconcile the topbar layer's mapping and the screencopy colour-match
    /// against the current fullscreen state. While a fullscreen client is active
    /// (and the overview isn't up), the bar is fully UNMAPPED — a null buffer,
    /// not just an empty frame — and the 700 ms colour-match capture is PAUSED,
    /// so the compositor can hand the fullscreen client direct-scanout /
    /// solitary and stop compositing our overlay entirely. Measured on the 2013
    /// Air: this is the difference between ~26 % waverunner CPU (blocking
    /// scanout, +18 °C, dropped frames) and ~0 % during fullscreen video. On
    /// exit it remaps and resumes.
    pub(crate) fn reconcile_options_fullscreen(&mut self) {
        if self.options_paused() {
            // A fullscreen client is up: PAUSE the screencopy colour-match
            // entirely (abort any in-flight capture, drop the target). This is
            // the continuous GPU readback Hyprland flags as "screen
            // record/screenshot" blocking direct-scanout — the biggest per-frame
            // cost waverunner adds during fullscreen video. The topbar itself
            // stays mapped but draws only a transparent frame (see draw_options),
            // so it costs a trivial single-rect render, not the capture.
            self.abort_capture();
            self.options_match = None;
            self.options_bar_matched = None;
        } else {
            // Back from fullscreen: resume the colour-match cadence.
            self.schedule_options_poll();
            self.reeval_options_bar();
        }
    }

    /// Whether the topbar's screencopy colour-match should be paused: a
    /// fullscreen client is focused and the overview (which shows the bar on its
    /// own strip) isn't up.
    pub(crate) fn options_paused(&self) -> bool {
        self.options_fullscreen && !self.overview_active
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
        } else if self.notif.expanded || self.clip.expanded || self.media_box_open {
            // Extend the pointer-sensitive region down over whichever box is
            // open so scroll/hover/clicks there reach us instead of passing
            // through. Use the *fully-expanded* bottom (not the live animating
            // height) so the region is stable the instant a box opens.
            let mut bottom = self.config.options.height as f32;
            if self.notif.expanded {
                bottom = bottom.max(self.notif_input_bottom());
            }
            if self.clip.expanded {
                bottom = bottom.max(self.clip_input_bottom());
            }
            if self.media_box_open {
                bottom = bottom.max(self.media_input_bottom());
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
                // A live drag on a media-box slider tracks the pointer.
                if self.media_drag_update(surface_x as f32) {
                    return;
                }
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
            // Left press: start a media-slider drag if it landed on a bar.
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_LEFT
                    && state == WEnum::Value(wl_pointer::ButtonState::Pressed)
                    && !self.options_hidden =>
            {
                if let Some((px, py)) = self.options_ptr {
                    self.media_drag_start(px, py);
                }
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_LEFT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released)
                    && !self.options_hidden =>
            {
                // A slider drag commits on release and swallows the click.
                if self.media_drag_commit() {
                    return;
                }
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
                && (matches!(
                    self.options_hover,
                    Some(PillId::Clipboard | PillId::ClipboardBox)
                ) || self.clip.expanded) =>
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

    pub(crate) fn options_update_hover(&mut self) {
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
            let (nt, moving) = ease_toward(self.options_ctrl.t[i], target, dt, CTRL_RATE, CTRL_EPS);
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
        // The open media transport box handles its own hits (transport / seek /
        // volume) at the pointer position.
        if self.media_box_open {
            if let Some((px, py)) = self.options_ptr {
                if self.media_box_click(px, py) {
                    self.draw_options();
                    return;
                }
                // A click outside the box closes it.
                self.media_box_open = false;
                self.sync_options_input();
                self.draw_options();
                // Fall through so the click can still hit a pill.
            }
        }
        match self.options_hover {
            // The media glyph pill toggles the transport box.
            Some(PillId::MediaOpen) => {
                self.media_box_open = !self.media_box_open;
                self.sync_options_input();
                self.draw_options();
            }
            // While the overview owns the screen the X closes the OVERVIEW,
            // not the window under it — the bar is the overview's only
            // on-screen exit affordance (Esc/Super+R being the others).
            Some(PillId::Close) if self.overview_active => hypr::close_overview(),
            Some(PillId::Close) => {
                if let Some(addr) = self.options_active_addr.clone() {
                    hypr::close_window(&addr);
                }
            }
            // The current-task pill cycles focus through this workspace's
            // windows, most-used first (see `crate::focus_cycle`).
            Some(PillId::Window) => self.cycle_focus(true),
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
            // A dynamic OPTION control from the Mind — run its action.
            Some(PillId::Option(i)) => self.trigger_option(i as usize),
            _ => {}
        }
    }

    /// Run the action of the `idx`-th actionable OPTION offer (the Mind's
    /// ranked controls). Called from a pill click and from the `options-trigger`
    /// IPC verb (scripting / verification).
    pub(crate) fn trigger_option(&mut self, idx: usize) {
        // Take an owned copy so the immutable borrow of the option set ends
        // before we run the (mutable-self) action.
        let picked = self
            .surfaced_options()
            .get(idx)
            .map(|a| (a.id.to_string(), a.action.clone()));
        if let Some((id, action)) = picked {
            info!("options: trigger '{id}'");
            self.run_affordance_action(&action);
        } else {
            warn!("options: trigger index {idx} out of range");
        }
    }

    /// Run an OPTION offer by its affordance id (the `options-trigger <id>` IPC
    /// verb). Returns whether an actionable offer with that id was found.
    pub(crate) fn trigger_option_by_id(&mut self, id: &str) -> bool {
        let action = self
            .surfaced_options()
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.action.clone());
        match action {
            Some(action) => {
                info!("options: trigger '{id}' (by id)");
                self.run_affordance_action(&action);
                true
            }
            None => {
                warn!("options: no actionable offer with id '{id}'");
                false
            }
        }
    }

    /// Execute an [`options_engine::AffordanceAction`]. The engine describes the
    /// action declaratively; this is where it becomes a real effect. Spawns are
    /// fully detached (double-fork via [`crate::launch`]); the argv is
    /// shell-quoted per element, so a path or URL with spaces/metacharacters is
    /// safe (the argv itself comes from the engine, never raw user text).
    fn run_affordance_action(&mut self, action: &options_engine::AffordanceAction) {
        use options_engine::AffordanceAction as A;
        match action {
            A::None => {}
            A::Spawn { .. } | A::OpenUrl(_) => {
                if let Some(line) = action_command_line(action) {
                    if let Err(e) =
                        crate::launch::launch(&line, false, &self.config.launch.terminal)
                    {
                        warn!("options: action spawn failed ({line}): {e:#}");
                    }
                }
            }
            A::HyprDispatch(cmd) => crate::hypr::dispatch(cmd),
            // Internal daemon actions, mapped by tag to a shell capability.
            A::Daemon(tag) => match tag.as_str() {
                "toggle_dnd" => self.toggle_notif_mute(),
                // A compositor keystroke to the focused window (no extra dep,
                // same path as the clipboard paste).
                "find_in_page" => crate::hypr::send_shortcut_active("CTRL", "f"),
                "reopen_tab" => crate::hypr::send_shortcut_active("CTRL SHIFT", "t"),
                "slide_next" => crate::hypr::send_shortcut_active("", "Right"),
                "slide_prev" => crate::hypr::send_shortcut_active("", "Left"),
                "present" => crate::hypr::send_shortcut_active("", "F5"),
                // XKB names: "Next" = PageDown, "Prior" = PageUp.
                "page_next" => crate::hypr::send_shortcut_active("", "Next"),
                "page_prev" => crate::hypr::send_shortcut_active("", "Prior"),
                // "pkgsearch:<name>" — open the launcher's Install search
                // pre-filled with a package name (a command-not-found remedy).
                t if t.starts_with("pkgsearch:") => self.pkg_search_for(&t["pkgsearch:".len()..]),
                other => warn!("options: unknown daemon action '{other}'"),
            },
        }
    }

    /// Open the launcher's Install search pre-filled with `query` — the
    /// command-not-found → install remedy. Opening (Target::Open) also kicks the
    /// lazy package index load, so results populate as soon as it is ready.
    fn pkg_search_for(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        self.search.query = q.to_string();
        // Open the full card. `Expand` only grows Dock→Open (a no-op from the
        // Hidden state the launcher sits in while another window is focused);
        // `Toggle` opens straight from Hidden or Dock. Guard so an already-open
        // launcher isn't toggled shut.
        if self.ui.target() != crate::state::Target::Open {
            self.handle_command(waverunner_proto::Command::Toggle);
        }
        self.refilter();
    }

    /// Right-click: over an open clipboard row, open its metadata detail view.
    /// On the current-task pill: cycle focus into the OTHER workspaces'
    /// windows, most-used first (the cross-workspace bounce).
    fn options_right_click(&mut self) {
        if self.clip.expanded && self.options_hover == Some(PillId::ClipboardBox) {
            self.clip_box_right_click();
            return;
        }
        if self.options_hover == Some(PillId::Window) {
            self.cycle_focus(false);
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
            // A dynamic OPTION pill is clickable only if it's an actionable
            // control — a privacy/safety WARNING pill is a passive indicator,
            // so it keeps the default cursor.
            Some(PillId::Option(i)) => {
                if self
                    .surfaced_options()
                    .get(i as usize)
                    .is_some_and(|a| a.action.is_actionable())
                {
                    Shape::Pointer
                } else {
                    Shape::Default
                }
            }
            // The small clipboard pill is clickable (paste) → pointer.
            Some(PillId::Clock) | None => Shape::Default,
            Some(_) => Shape::Pointer, // control circle / small clipboard pill
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use options_engine::AffordanceAction;

    #[test]
    fn option_glyphs_map_by_id_and_playpause_by_title() {
        assert_eq!(glyph_for_option("media.playpause", "Pause"), GLYPH_PAUSE);
        assert_eq!(glyph_for_option("media.playpause", "Play"), GLYPH_PLAY);
        assert_eq!(glyph_for_option("media.vol_up", ""), GLYPH_VOL_UP);
        assert_eq!(glyph_for_option("git.commit", ""), GLYPH_COMMIT);
        assert_eq!(glyph_for_option("git.push", ""), GLYPH_PUSH);
        assert_eq!(glyph_for_option("audio.mic_mute", ""), GLYPH_MIC_SLASH);
        assert_eq!(glyph_for_option("selection.url", ""), GLYPH_COPY_LINK);
        // An unknown id still gets a (generic) glyph, never a crash.
        assert_eq!(glyph_for_option("something.new", ""), GLYPH_OPTION);
    }

    #[test]
    fn action_command_line_shell_quotes_argv() {
        // A repo path with a space is passed literally, not word-split.
        let a = AffordanceAction::Spawn {
            argv: vec![
                "git".into(),
                "-C".into(),
                "/home/max/my repo".into(),
                "commit".into(),
                "-am".into(),
                "Update (via OPTIONS)".into(),
            ],
        };
        // shell_quote wraps every element, so a space in the path is safe.
        assert_eq!(
            action_command_line(&a).unwrap(),
            "'git' '-C' '/home/max/my repo' 'commit' '-am' 'Update (via OPTIONS)'"
        );
        // A URL with shell metacharacters cannot break out of its argument.
        let u = AffordanceAction::OpenUrl("https://x.test/a?b=1&c=$(rm -rf ~)".into());
        assert_eq!(
            action_command_line(&u).unwrap(),
            "xdg-open 'https://x.test/a?b=1&c=$(rm -rf ~)'"
        );
        // None / empty spawn produce no command line.
        assert_eq!(action_command_line(&AffordanceAction::None), None);
        assert_eq!(
            action_command_line(&AffordanceAction::Spawn { argv: vec![] }),
            None
        );
    }

    /// The bar's resting wash for a dark (unmatched) bar — what the box
    /// composites over its backdrop.
    fn dark_bar_wash() -> [f32; 4] {
        wash(true, 0.11)
    }

    #[test]
    fn ink_reads_on_whatever_it_sits_on() {
        // A light wallpaper behind the transparent bar takes dark ink — the
        // case that was unreadable while the bar used a static theme white.
        assert_eq!(ink_on([0.7, 0.75, 0.8, 1.0]), INK_DARK);
        assert_eq!(ink_on([0.05, 0.06, 0.08, 1.0]), INK_LIGHT);
    }

    #[test]
    fn hover_strengthens_the_ink_it_never_fades_it() {
        // Hover takes the ink to full strength and leaves its COLOUR alone:
        // weight carries the emphasis, so moving the colour as well made the
        // hover heavy. It must never fade — that was the original defect,
        // where lightening dark ink pushed it toward a light background.
        for (ink, rest) in [(INK_DARK, 0.88), (INK_LIGHT, 0.67)] {
            let hov = hover_ink_for([ink[0], ink[1], ink[2], rest]);
            assert_eq!([hov[0], hov[1], hov[2]], [ink[0], ink[1], ink[2]]);
            assert!(hov[3] > rest, "hover must gain strength, not lose it");
        }
    }

    #[test]
    fn ink_is_softened_and_warm() {
        for ink in [INK_LIGHT, INK_DARK] {
            // Never pure black or white...
            assert!(ink[0] > 0.0 && ink[0] < 1.0);
            // ...and warm: red leads, blue trails.
            assert!(
                ink[0] > ink[1] && ink[1] > ink[2],
                "ink {ink:?} is not warm"
            );
        }
        // Still separated far enough to carry contrast on either surface.
        assert!(luminance(INK_LIGHT) > 0.6 && luminance(INK_DARK) < 0.05);
    }

    #[test]
    fn box_and_bar_reach_the_same_ink_on_one_backdrop() {
        // The original defect: the bar said white while both boxes said black
        // over the same wallpaper. The box's fill is now that backdrop plus a
        // weak wash, so measuring each independently must agree. (Samples are
        // kept off the 0.179 flip point, where a wash CAN legitimately tip
        // one side.)
        for backdrop in [
            [0.70, 0.72, 0.66, 1.0], // the cream wallpaper
            [0.13, 0.35, 0.56, 1.0], // the blue sky
            [0.02, 0.02, 0.03, 1.0], // a dark window
        ] {
            let fill = box_fill(backdrop, dark_bar_wash());
            assert_eq!(
                ink_on(fill),
                ink_on(backdrop),
                "box and bar disagreed on {backdrop:?}"
            );
        }
    }

    #[test]
    fn box_fill_stays_close_to_the_backdrop() {
        // "A similar color to the bg": the wash may not drag the fill far
        // from what it floats on, and must not flatten its chroma.
        let backdrop = [0.13, 0.35, 0.56, 1.0];
        let fill = box_fill(backdrop, dark_bar_wash());
        for i in 0..3 {
            assert!(
                (fill[i] - backdrop[i]).abs() < 0.12,
                "channel {i} drifted: {} vs {}",
                fill[i],
                backdrop[i]
            );
        }
        // Chroma survives: still clearly blue, not pulled toward grey.
        assert!(fill[2] / fill[0] > 2.5);
    }
}

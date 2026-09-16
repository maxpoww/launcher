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

use crate::animation::{self, ease_toward, lerp, lerp4};
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
pub(crate) const TEXT_FONT: Option<&str> = None;
/// Colour emoji must be asked for BY NAME. The sans-serif fallback chain
/// resolves the smiley block (U+1F600…) to **DejaVu Sans**, which carries
/// monochrome outlines for it and wins before the colour font is ever reached —
/// so 🎉 (absent from DejaVu) came out in colour while 😀 came out as a black
/// ring (seen live, 2026-09-13). Naming the family skips the chain.
pub(crate) const EMOJI_FONT: &str = "Noto Color Emoji";
pub(crate) const FONT_PX: f32 = 17.0;
pub(crate) const LINE_PX: f32 = 20.0;

/// Pure small-screen scale mapping for the topbar (pills, bar height, boxes):
/// full size on panels at or above `OPTIONS_FULL_H` logical px, easing down
/// linearly to `OPTIONS_MIN_SCALE` on short ones. `None` (output size not yet
/// known) means full size. [`crate::App::options_scale`] is this, fed the live
/// output height — kept pure here so the mapping is unit-testable.
pub(crate) fn pill_scale_for(logical_h: Option<f32>) -> f32 {
    const OPTIONS_FULL_H: f32 = 900.0;
    const OPTIONS_MIN_SCALE: f32 = 0.82;
    match logical_h {
        Some(h) if h < OPTIONS_FULL_H => (h / OPTIONS_FULL_H).max(OPTIONS_MIN_SCALE),
        _ => 1.0,
    }
}

/// Split an `overview-hover` payload into the window it names and the title to
/// show.
///
/// waveview sends `"0x55f3… Some Window Title"`: the pill only ever needed the
/// words, but the stage needs to know *which* window they belong to, since
/// entering from the map stages what the pointer is on. An empty payload ends
/// the hover. A payload with no address is read as a bare title, so an older
/// plugin still drives the pill rather than putting its own text in the wrong
/// field.
pub(crate) fn split_overview_hover(payload: &str) -> (Option<String>, Option<String>) {
    if payload.is_empty() {
        return (None, None);
    }
    match payload.split_once(' ') {
        Some((addr, title)) if addr.starts_with("0x") => (
            Some(addr.to_owned()),
            (!title.is_empty()).then(|| title.to_owned()),
        ),
        // An address and nothing else: a window with no title yet.
        None if payload.starts_with("0x") => (Some(payload.to_owned()), None),
        _ => (None, Some(payload.to_owned())),
    }
}

/// Multi-output precedence for the shell's scale source: the logical height of
/// the output the shell's own surface actually maps to (learned from
/// `wl_surface` enter) wins; the first enumerated output is only the fallback
/// before any enter arrives (or after a leave). On a laptop + external setup
/// enumeration order says nothing about where the bar lives — the compositor's
/// placement does.
pub(crate) fn preferred_output_height(entered: Option<f32>, first: Option<f32>) -> Option<f32> {
    entered.or(first)
}

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

// --- Sunset prompt ----------------------------------------------------------
// The current-task pill as an INTERACTING MODULE. At sunset the Mind offers
// eye protection (`sunset.eye_protection`, from the engine's daylight layer:
// timezone → coordinates → live solar elevation). Instead of a glyph in the
// OPTION cluster, the offer takes over the window pill: the title metamorphoses
// into the question and a [turn on] pill rests INSIDE the module's right end.
// One shape growing (the `become-more` flow) — not a popup, not a new pill.
// [turn on] warms the screen (hyprsunset); a right-click on the module is
// "not now". Either way the pill morphs back to the task it was showing.
/// The message the module shows. Its own constant (never truncated) — it is
/// also the key the morph fade uses to know the prompt is what's on the pill.
pub(crate) const SUNSET_MSG: &str = "The sun is set, do you want to turn on eye protection?";
const SUNSET_TURN_ON_LABEL: &str = "turn on";
/// What [turn on] asks for (Kelvin) — the evening warmth the offer promises,
/// and one of the panel's presets so the box agrees with the prompt.
const SUNSET_TURN_ON_K: u32 = 4000;
/// How the answered offer is titled on its notification record when there is no
/// live affordance to read the title from (a debug-forced prompt). Matches the
/// engine's own `sunset.eye_protection` title, so the record reads the same
/// either way.
const SUNSET_OFFER_TITLE: &str = "Turn on eye protection";
/// Gap between the two nested pills ([turn on] and the settings gear).
const SUNSET_INNER_GAP: f32 = 4.0;
/// The settings box's width (logical px, pre-scale). The module morphs from its
/// wide message pill to this centred panel — narrowing to a sensible settings
/// width and dropping to its own height ([`Module::box_size`]), like the
/// notif/clipboard boxes. Shared by every module: the panels are the same
/// object seen twice, and a width that varied per module would make them two.
pub(crate) const MODULE_BOX_W: f32 = 340.0;
/// The open box's corner radius — a rounded RECTANGLE like the dock's open
/// card (`BOX_CORNER_RADIUS` = 24), only smaller for this compact panel.
pub(crate) const MODULE_PANEL_RADIUS: f32 = 24.0;
/// The blister radius (logical px, pre-scale): how far the banner's swell
/// fillets as it bulges around the module. The blister rect's quad is expanded
/// by this so the bulge has room; the shader insets the SDF back. Fed to
/// `Scene::neck`.
pub(crate) const MODULE_NECK_K: f32 = 14.0;
/// How far the banner-behind layer extends past the glass pill on each side —
/// the sliver of banner that shows around the pill, so the pill reads as
/// sitting ON the banner (the banner is a distinct layer behind it).
const MODULE_BANNER_RIM: f32 = 9.0;
/// The `glass` sentinel flagging a rect as the banner blister (solid fill of
/// the bar colour, smooth-unioned with the bar edge — see `rounded_rect.wgsl`).
const MODULE_NECK_GLASS: f32 = 2.0;
/// The engine affordance id the prompt is the surface for.
pub(crate) const SUNSET_OFFER_ID: &str = "sunset.eye_protection";
/// Gap between the message text and the nested [turn on] pill.
const MODULE_GAP: f32 = 9.0;
/// The asking module's size step past a normal pill — a REAL step (the +2px
/// hover lift is a whisper, and this must not read like one), while the
/// nested [turn on] stays exactly normal-pill sized inside it: the size
/// difference is what makes the nesting legible. The extra height hangs
/// DOWNWARD: the top edge drops a couple of pixels clear of the screen edge
/// and the bottom reaches a little past the bar line, the way the bell's
/// preview pill steps out of the bar.
const MODULE_GROW_X: f32 = 6.0;
pub(crate) const MODULE_GROW_H: f32 = 13.0;
const MODULE_DROP_Y: f32 = 2.0;
/// Alpha of the nested [turn on] pill's hairline border, in the module's own
/// ink — a whisper, just enough to seat the button in the shared glass.
const SUNSET_BORDER_A: f32 = 0.06;
/// How far the nested [turn on] grows past a normal pill, per side — a little
/// bigger so it fills the enlarged module rather than looking dwarfed in it.
const MODULE_CHILD_GROW: f32 = 1.5;
/// How much more opaque the asking module's RESTING fill reads than an
/// ordinary pill's wash (Max, 2026-09-08: "rise the pill opacity" — the
/// message pill was reading almost as transparent as the banner behind it).
/// Multiplies `options_rest_wash`'s alpha for this module only — every other
/// pill keeps the shared wash exactly as it was. Capped at the open box's own
/// alpha so the closed pill never reads MORE solid than the panel it grows
/// into.
const MODULE_REST_ALPHA_BOOST: f32 = 10.0;

/// A module that takes the current-task pill over: one wide asking pill where
/// the window title was, on its banner blister, with its own costume riding the
/// title metamorphosis (`become-more`, one shape growing).
///
/// **One at a time** — the pill is one object, so it can only be one thing
/// (`OptionUXRules.md` §5, one kills the other). [`App::module_wanted`] picks by
/// precedence rather than stacking two sentences into one pill.
///
/// An **empty-space** module lived here on 2026-09-13 — the plain sentence "This
/// space is empty" on any workspace with no windows, with the gear and a box of
/// its own — and Max cut it the same day, on sight: *"i meant the whole pill…
/// all, get rid of it."* The generalisation it forced is what remains: the
/// costume below is no longer sunset-specific, so the next module is a variant
/// and a match arm rather than a rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Module {
    /// The sunset offer: "…turn on eye protection?" + [turn on] + the gear.
    Sunset,
}

impl Module {
    /// Every module, in the order they claim the pill (see
    /// [`App::module_wanted`]).
    const ALL: [Module; 1] = [Module::Sunset];

    /// The module's **fixed** sentence: sunset's question, and the empty room's
    /// fallback line for when it has nothing to report.
    ///
    /// What is actually drawn goes through [`App::module_line`], because a
    /// module's sentence can be live — the empty room says whatever is most
    /// worth saying. The identity of the drawn text travels beside it in
    /// [`TitleMeta::shown_module`], not by comparing strings.
    pub(crate) const fn msg(self) -> &'static str {
        match self {
            Module::Sunset => SUNSET_MSG,
        }
    }

    /// Whether the module nests an action pill — sunset's `[turn on]`, the
    /// button that answers its question. A module with nothing to answer has
    /// none.
    fn has_turn_on(self) -> bool {
        matches!(self, Module::Sunset)
    }

    /// Whether the module carries a settings gear (and therefore a box behind
    /// it) at its right end.
    ///
    /// Asked per module rather than assumed: a gear is a promise that there is
    /// something to set, and one that opens an empty panel is a toll. Both
    /// modules answer yes today — Max, 2026-09-13: *"now add a gear pill inside
    /// as the one on sunset, and make it open a box as sunset does."*
    fn has_gear(self) -> bool {
        matches!(self, Module::Sunset)
    }

    /// Whether anything is nested in the module's right end. When something is,
    /// the sentence is LEFT-anchored and clipped at the leftmost child's edge;
    /// a childless module centres its sentence like an ordinary title.
    fn has_children(self) -> bool {
        self.has_turn_on() || self.has_gear()
    }

    /// The settings box's fully-open size (logical px, pre-scale). The width is
    /// shared so every module's panel is the same object seen twice; the height
    /// is the module's own, because a panel sized for content it does not have
    /// reads as broken rather than as room to grow.
    fn box_size(self) -> (f32, f32) {
        match self {
            // Header + the temperature row + the "automatically" toggle.
            Module::Sunset => (MODULE_BOX_W, 168.0),
        }
    }

    /// What the settings panel calls itself in its header band — the module
    /// named as a thing you can set, where the sentence was. Not the sentence
    /// itself: "The sun is set, do you want to…?" is a question, and a panel
    /// header is a place.
    pub(crate) fn panel_title(self) -> &'static str {
        match self {
            Module::Sunset => "Eye protection",
        }
    }

    /// The stable lower-case name, for the log line that records a hand-over.
    fn name(self) -> &'static str {
        match self {
            Module::Sunset => "sunset",
        }
    }
}

// Nerd Font glyphs (Font Awesome range, present in JetBrainsMono NF).
pub(crate) const GLYPH_CLOSE: &str = "\u{f00d}"; // fa-times
const GLYPH_SQUARE: &str = "\u{f096}"; // fa-square-o (pseudotile)
const GLYPH_FULL: &str = "\u{f065}"; // fa-expand (fullscreen)
const GLYPH_FLOAT: &str = "\u{f2d2}"; // fa-window-restore (floating)
pub(crate) const GLYPH_BELL: &str = "\u{f0f3}"; // fa-bell (notification OPTION)
pub(crate) const GLYPH_BELL_SLASH: &str = "\u{f1f6}"; // fa-bell-slash (mute pill)
pub(crate) const GLYPH_CLIPBOARD: &str = "\u{f0ea}"; // fa-clipboard (clipboard OPTION)
pub(crate) const GLYPH_COPY: &str = "\u{f0c5}"; // fa-copy (detail-view copy)
pub(crate) const GLYPH_COPY_LINK: &str = "\u{f0c1}"; // fa-link (copy the page URL)

// Dynamic OPTION-pill glyphs (the Mind's context-aware controls). Keyed by
// affordance id in `glyph_for_option`.
/// The deck's audible-task speaker (with waves, like a browser tab's) — pure
/// status, no click behaviour.
pub(crate) const GLYPH_VOL_LIVE: &str = "\u{f028}"; // fa-volume-up
const GLYPH_OPTION: &str = "\u{f0eb}"; // fa-lightbulb-o (generic OPTION)
                                       // The cava cluster's transport glyphs.
const GLYPH_PLAY: &str = "\u{f04b}"; // fa-play
const GLYPH_PAUSE: &str = "\u{f04c}"; // fa-pause
const GLYPH_PREV: &str = "\u{f048}"; // fa-step-backward
const GLYPH_NEXT: &str = "\u{f051}"; // fa-step-forward
const GLYPH_GEAR: &str = "\u{f013}"; // fa-cog (settings — sunset module)
/// The stage-mode pill's two faces: one task alone, or a whole desk.
const GLYPH_ONE_TASK: &str = "\u{f2d0}"; // fa-window-maximize
const GLYPH_WHOLE_DESK: &str = "\u{f009}"; // fa-th-large
/// Amber wash for a privacy/safety WARNING pill, so it reads as "heads up",
/// not a button.
const WARN_COLOR: [f32; 4] = [1.0, 0.72, 0.30, 1.0];

/// Whether an affordance is surfaced as an OPTION pill: any actionable control,
/// plus the privacy/safety WARNINGS worth a persistent glance (a live camera,
/// mic, or screen share). Battery/deploy warnings are excluded — they have
/// their own dedicated surfaces (battery.rs, and the deploy nudge).
///
/// The amber WARNING allow-list that used to sit here named five deleted
/// affordance ids (`camera.live`, `audio.mic_live`, …) and went with them on
/// 2026-09-12. It comes back when the first curated Warning does — the rule it
/// encoded stands: battery and deploy stay excluded because they have their own
/// dedicated surfaces (`battery.rs`, the deploy nudge).
pub(crate) fn is_surfaced_affordance(a: &options_engine::Affordance) -> bool {
    // A module offer is excluded like battery/deploy: its surface is the
    // current-task pill, not a cluster glyph. (`space.empty` carries no action
    // yet, so it would fall out below anyway — named here so that giving it one
    // cannot silently sprout a second, generic pill for the same offer.)
    a.action.is_actionable() && a.id != SUNSET_OFFER_ID
}

/// The Nerd-Font glyph for a dynamic OPTION control, by affordance id.
///
/// **Empty seam.** The 44 id→glyph arms went with the uncurated offers
/// (2026-09-12) — every one of them named a deleted affordance. Each curated
/// OPTION adds its own arm back as it is built. The play/pause pattern is the
/// one worth remembering: it read its `title` so the glyph showed the action it
/// WOULD perform, not the state it was in.
fn glyph_for_option(_id: &str, _title: &str) -> &'static str {
    GLYPH_OPTION
}

/// How many dynamic OPTION pills the topbar shows at once — a de-cluttered
/// cluster that stays clear of the centred window pill.
const OPTION_PILL_CAP: usize = 5;

/// The cava pill's width, in pill-heights. Wider than a glyph circle because
/// it holds [`crate::spectrum::BANDS`] bars; narrow enough that it reads as a
/// pill rather than a panel. Widened from 1.9 on 2026-09-12 — at seven bands
/// the bars were hairlines.
const CAVA_PILL_W: f32 = 2.6;
/// Hit-width of one transport symbol inside the now-playing pill. Wider than
/// the glyph itself so a click does not demand precision the symbol's ink
/// implies.
const CAVA_SYM_W: f32 = 17.0;
/// Space between the three symbols.
const CAVA_SYM_GAP: f32 = 3.0;
/// Below this, an axis event is the tail of a flick or the stop that ends one —
/// not somebody asking for something.
pub(crate) const SCROLL_DEADZONE: f32 = 0.5;
/// Space between the track name and the trailing time.
const CAVA_TIME_GAP: f32 = 10.0;
/// Extra breathing room in the now-playing pill (Max: "make the pill wider").
const CAVA_NOW_EXTRA: f32 = 44.0;
/// Ceiling on the now-playing pill, in pill-heights — so it scales with the bar
/// exactly like [`CAVA_PILL_W`] rather than being a fixed pixel count that goes
/// wrong on a small screen.
///
/// Without a cap the pill is however long the track name is, and a 90-character
/// title would push the whole OPTION band off toward the window pill. Set just
/// above the width a typical track needs (~21 characters at `FONT_PX`), so it
/// only ever bites on the long ones — which then clip, because the label is
/// bounded to its own pill.
const CAVA_NOW_MAX_W: f32 = 13.0;

/// Marquee speed for a track name too long for its pill, in logical px/s. Slow
/// enough to read at a glance rather than chase.
///
/// **This is a rate, and §3 says a module declares no rate of its own** —
/// flagged like the spectrum's decay envelope rather than hidden. It is not a
/// morph (`animation.rs` owns those) and not a leave-hold; it is closest to
/// §3's named exception, the auto-withdraw dwell, in that it answers *"how long
/// does this need to be readable"* rather than *"is the user still here"*.
/// Max's call whether it joins the shared vocabulary.
const CAVA_SCROLL_SPEED: f32 = 34.0;
/// The empty run between the end of the title and where it begins again.
///
/// The marquee is a **loop, not a there-and-back** (Max, 2026-09-12): the text
/// is drawn twice, a period apart, and the pair slides forever. The gap is what
/// makes the seam read as "it has come round again" rather than as one endless
/// sentence — wide enough to be a pause, narrow enough that the pill is never
/// just blank.
const CAVA_SCROLL_GAP: f32 = 34.0;

/// How long a track change takes to play out. Short — it is a change of
/// subject, not a journey — but long enough that the eye registers the old
/// words leaving rather than finding different ones already there.
pub(crate) const CAVA_SWAP_SECS: f32 = 0.34;

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
// The stagger is CHOREOGRAPHY, not tempo: One Material shares the rate, never
// the moment (`OptionUXRules.md` §3).
const CTRL_STAGGER: f32 = 0.06; // s between stagger stages
/// How many toggles ride the reveal chain.
const CTRL_N: usize = 3;
/// How long to let a window settle into its tile before measuring it — the
/// second beat of a fullscreen/float → pseudo transition. Comfortably past the
/// compositor's move animation, since a size read mid-flight is worse than a
/// slightly later one.
const MODE_SETTLE: Duration = Duration::from_millis(500);

/// How long the solitary-pseudo rule waits after a window opens or closes.
///
/// Just long enough for the compositor to have finished updating its own client
/// list — the rule needs an accurate *count*, not a settled geometry, and the
/// size it works from is computed rather than measured
/// ([`hypr::solitary_tile`]). Short enough that the window is still animating
/// in when the pseudo lands, so it grows straight into Golem's proportions
/// instead of reaching the tile and then shrinking (Max, 2026-09-13: "it should
/// be instant"). It also coalesces a burst — an app opening three windows runs
/// one sweep.
const LAYOUT_SETTLE: Duration = Duration::from_millis(60);

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
// full date; it lets go a beat after the pointer leaves the surface, then plays
// the same transition backwards. All dt-based.
//
// That beat is the surface's shared `animation::LEAVE_HOLD`, not the clock's own
// idea of one (`OptionUXRules.md` §3). It used to be 1500ms, which left the date
// standing long after the hand that asked for it had gone: if you want to keep
// reading it, keep the pointer on it.
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

// --- The Leader (OptionUXRules.md §1) ---------------------------------------
// "Using an OPTION must never cost you the OPTION." A CLICK chooses the LEADER:
// the pill you clicked is pinned to the place on the bar where you clicked it,
// and its group is laid out from that anchor instead of from its resting one.
// A pill that changes width therefore spends the change on its far side. Close
// a tile holding [X] and the shorter title spends its whole width change on its
// left edge — [X] stays put and you can click again without re-aiming. The
// anchor holds until the pointer leaves the bar, then the group eases home; at
// rest displacement is exactly 0, so the resting layout is the one that was
// always there.
//
// The anchor is a PLACE ON THE BAR, never the pointer. An earlier cut of this
// rule captured on hover and solved for "the leader lands under the cursor",
// which meant the cluster travelled with the pointer: setting off from
// [current task] toward [X] pushed [X] away at exactly the speed it was
// chased, and it could never be reached. A leader you must be able to aim at
// cannot be attached to your aim.

/// The pill groups that lay out as a unit. Only the two that actually re-flow
/// can be led: the window cluster (the title's width moves the controls) and
/// the Mind's ranked control row (offers arrive and withdraw). The clock, the
/// bell and the clipboard are edge-pinned and own their morphs, so leading them
/// would fight animations that are already correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PillGroup {
    /// `[current task] [X] [pseudo] [fullscreen]` — centred, re-flows on title.
    Window,
    /// The Mind's context-aware controls + the media opener — left-anchored,
    /// re-flows as offers are ranked in and out.
    Mind,
    /// The gear at the bar's left end. Alone, and pinned to the edge: it is the
    /// one pill whose place never depends on anything else, which is what makes
    /// it findable without looking.
    Settings,
    Clipboard,
    Notif,
    Clock,
}

/// How many groups can be led — the width of [`LeadAnim::off`].
const LEAD_N: usize = 2;

/// Which pills move together. The one table that says what "its OPTION" means
/// when the rule says the group lays out from the leader.
pub(crate) fn group_of(id: PillId) -> PillGroup {
    match id {
        PillId::Window
        | PillId::SunsetTurnOn
        | PillId::ModuleSettings
        | PillId::Close
        | PillId::WindowState
        | PillId::Pseudo
        | PillId::Float
        | PillId::Fullscreen => PillGroup::Window,
        PillId::Option(_) => PillGroup::Mind,
        // The stage's mode switch leads the same band: it is not about the
        // focused window either, and it arrives and withdraws with the mode
        // exactly as the Mind's offers do.
        PillId::StageMode => PillGroup::Mind,
        PillId::Cava
        | PillId::CavaPlay
        | PillId::CavaPrev
        | PillId::CavaNext
        | PillId::CavaNow
        | PillId::CavaOut => PillGroup::Mind,
        PillId::Settings | PillId::SettingsStats => PillGroup::Settings,
        PillId::Clipboard | PillId::ClipboardBox | PillId::ClipCopyLink => PillGroup::Clipboard,
        PillId::Notif | PillId::NotifMute => PillGroup::Notif,
        PillId::Clock => PillGroup::Clock,
        // A doorway belongs to whatever it stands in for; today only the window
        // controls can go sticky. It never reflows anyway — a sticky pair is a
        // fixed two-pill layout — so its group is only ever asked for in passing.
        PillId::Doorway => PillGroup::Window,
    }
}

/// Displacement slot for a leadable group, `None` for the ones the rule does
/// not (yet) cover — the single gate deciding what can be led.
pub(crate) fn group_slot(g: PillGroup) -> Option<usize> {
    match g {
        PillGroup::Window => Some(0),
        PillGroup::Mind => Some(1),
        // The gear never re-flows — it is pinned to the edge, so there is
        // nothing for the Leader to hold still.
        PillGroup::Settings => None,
        PillGroup::Clipboard | PillGroup::Notif | PillGroup::Clock => None,
    }
}

/// The pill a click has pinned.
#[derive(Debug, Clone)]
pub(crate) struct Leader {
    /// The pill as it was captured. For a Mind control this is a *slot*, which
    /// is why `action` exists.
    id: PillId,
    /// "The leader is an identity, not a slot": for a Mind control, the
    /// affordance id it was captured on. If the ranking reorders, the leader
    /// follows the ACTION to its new slot; if the action withdraws, the leader
    /// is released — so a re-rank can never put a different action under a
    /// stationary finger.
    action: Option<&'static str>,
    /// The bar x the leader's centre is pinned to: where the pill sat at the
    /// moment it was clicked. Fixed for the life of the leader — the pointer
    /// may travel anywhere on the bar without moving it.
    anchor_cx: f32,
}

/// Per-group displacement from the resting layout, plus the ease home.
///
/// While a leader is held its group's displacement is *derived* every layout
/// (see [`App::live_lead`]) rather than stored — that is what pins the leader
/// through a layout change that happens between pointer events. Only the
/// release needs state: the last derived value is frozen here and eased to 0.
#[derive(Debug, Default)]
pub(crate) struct LeadAnim {
    /// Logical-px offset per leadable group; 0 = resting.
    off: [f32; LEAD_N],
    last: Option<std::time::Instant>,
    frame_pending: bool,
}

/// Where the leader's group must sit for the leader to stay on its anchor: the
/// pure core of the rule.
///
/// `nat` is the leader's rect in the *resting* layout — where it would be if
/// the rule did not exist. The group is translated by the difference, so a
/// leader whose pill did not move displaces nothing: at rest this is exactly 0
/// and the drawn layout is the resting one.
///
/// The anchor is the leader's CENTRE, so a leader that resizes holds its place
/// on the bar rather than one of its edges, spending the width change evenly
/// instead of lunging. For the fixed-width buttons that do the reflowing work
/// — `[X]`, the Mind's controls — centre and edges are the same promise: the
/// pill is bit-still.
///
/// Nothing here reads the pointer, and that IS the guarantee: the leader is a
/// fixed point you can travel to, not something that travels with you.
fn lead_shift(nat: Rect, anchor_cx: f32) -> f32 {
    anchor_cx - (nat.x + nat.w / 2.0)
}

/// "The leader yields to the edges": bound a group's shift so it cannot push
/// into its neighbours. `span` is the group's resting extent, `barrier_l` /
/// `barrier_r` the nearest neighbouring edges (`None` = the free bar edge, and
/// `edge_l`/`edge_r` bound that). When the clamp bites the leader *does* move —
/// correct geometry outranks the guarantee, and it breaks visibly.
fn clamp_shift(
    span: (f32, f32),
    barrier_l: Option<f32>,
    barrier_r: Option<f32>,
    edge: (f32, f32),
    gap: f32,
    raw: f32,
) -> f32 {
    let (gl, gr) = span;
    let lo = barrier_l.map_or(edge.0, |b| b + gap) - gl;
    let hi = barrier_r.map_or(edge.1, |b| b - gap) - gr;
    // A group already too wide for its slot inverts the range; pinning to `lo`
    // keeps it deterministic (and leaves the overflow where it always was).
    raw.clamp(lo, hi.max(lo))
}

// --- Sticky OPTIONS (OptionUXRules.md §4) ------------------------------------
// "Undoing must cost what doing cost." §1 keeps an OPTION from MOVING when you
// use it; this keeps one from DISAPPEARING when you use it. Clicking
// [fullscreen] conceals the bar — so the control you need in four seconds, to
// undo what you just did, was taken away by what you just did. Doing costs one
// click; undoing costs a journey down, up, a dwell and a hunt.
//
// So when an action takes the bar away, the OPTION that did it stays, alone,
// exactly where the hand left it — plus a DOORWAY: an empty pill standing in
// the slot its successor will occupy. Hover the doorway (no click, no dwell)
// and the whole bar returns, the real pill arriving in that same place.
//
// It is a grace, not a mode: nothing to dismiss, no timer. Move off the pair
// and it goes, and the bar is back to its ordinary rules.

/// The bar's own coming and going — One Material (`OptionUXRules.md` §3)
/// applied to the surface itself, not just to the pills on it.
///
/// The bar used to appear and vanish instantly, which made it the one thing up
/// here that did not obey the tempo: every OPTION glided, and the surface
/// carrying them snapped. It fades on the shared rate now, whichever way it is
/// summoned or dismissed — the top-edge dwell, a fullscreen taking the screen,
/// the pointer leaving, or a doorway opening.
#[derive(Debug)]
pub(crate) struct ShowAnim {
    /// 0 = gone, 1 = fully present. The drawn presence, not the intent —
    /// `options_hidden` is the intent, and interaction follows that at once
    /// rather than waiting for the fade.
    pub(crate) t: f32,
    last: Option<std::time::Instant>,
    frame_pending: bool,
}

impl Default for ShowAnim {
    /// The bar starts fully present: a session opens with it already there, not
    /// fading in from nothing.
    fn default() -> Self {
        Self {
            t: 1.0,
            last: None,
            frame_pending: false,
        }
    }
}

/// A control that took the bar away and stayed behind, with its way back.
#[derive(Debug, Clone)]
pub(crate) struct Sticky {
    /// The OPTION that did it. Still live: clicking it again undoes the thing.
    id: PillId,
    /// Where it sat when the bar went — the place §1 had pinned it to. Kept as
    /// an absolute rect, because the layout it belonged to no longer exists.
    rect: Rect,
    /// The doorway's rect: one slot along, on the side the rest of its group
    /// lives, sized like its successor because it is standing in for it.
    door: Rect,
    /// The reveal chain exactly as it stood when the bar went away.
    ///
    /// The doorway's promise is "the OPTIONS as they were when you pressed
    /// this" — so the bar it brings back must be the bar you left, not one
    /// rebuilding itself from scratch. Frozen here rather than left running,
    /// because a chain that keeps animating while concealed has already thrown
    /// away the state it is supposed to return to.
    ctrl: [f32; CTRL_N],
    ctrl_reveal: bool,
}

/// How long after acting on the bar a concealment still counts as *caused by*
/// that action. Past this the bar concealed for its own reasons (the pointer
/// wandered off, the compositor changed something) and nothing sticks.
const STICKY_BLAME: Duration = Duration::from_millis(1200);

// --- Title metamorphosis ----------------------------------------------------
// The Leader says the width change happens *away* from the leader; this is the
// width change itself. The window pill used to jump between title widths — now
// it eases, crossfading the outgoing name into the incoming one, exactly the
// clock↔date pattern. Holding [X], the pill's right edge is pinned against it
// and the whole ease plays out leftward.
/// Crossfade split, mirroring the clock's: the old name is gone by
/// `t = OUT_END`, the new one starts at `IN_START` — a slight overlap.
const TITLE_OUT_END: f32 = 0.55;
const TITLE_IN_START: f32 = 0.45;
/// How soon after a title change the SAME window retitling itself again counts
/// as churn rather than a new task (see [`App::title_churn`]). A window that
/// rewrites its title faster than this is running a counter, not changing what
/// you are doing — and a pill that crossfaded at that rate would be a strobe.
const TITLE_CHURN_GAP: Duration = Duration::from_millis(1200);

/// Whether the date may start collapsing back to the time: it is out, no
/// collapse is already armed, and the pointer has left the clock.
///
/// **Leaving the PILL is the trigger**, the same as every other element on this
/// bar (Max, 2026-09-13: *"the clock waits for me to abandon the banner before
/// it colapse, but the rest go away when i move out the pill"*). It used to
/// wait for the pointer to leave the whole surface, and that was right at the
/// time: the notification cluster was pinned to the clock's LIVE edge, so a
/// collapse on a timer slid the bell ~180px out from under a pointer that was
/// aiming at it — reaching for an OPTION cost you the OPTION. The cluster now
/// hangs off the clock's RESTING edge and is merely covered
/// ([`App::options_clock_rest_left`]), so a collapse moves nothing sideways; it
/// hands the bell back in the place it never left, and the reason to wait is
/// gone with it. The shared [`animation::LEAVE_HOLD`] still sits in front of
/// it, so brushing past the clock does not play the morph twice.
fn clock_may_collapse(showing_date: bool, collapse_pending: bool, on_clock: bool) -> bool {
    showing_date && !collapse_pending && !on_clock
}

/// Progress + endpoints for the window pill's title metamorphosis.
#[derive(Debug)]
pub(crate) struct TitleMeta {
    /// Content width the morph started from (the pill grows/shrinks from here
    /// toward the live measured width).
    from: f32,
    /// The name being faded out.
    outgoing: String,
    /// The name last measured — the current one; a change starts a morph.
    shown: String,
    /// Which module (if either) those two texts belong to.
    ///
    /// The costume (size step, banner, children, ink) has to know which module
    /// is on screen at every step of the morph, including the half where the old
    /// one is still fading out. It used to re-derive that by comparing the text
    /// against each module's fixed sentence, which only works while every
    /// sentence is a constant. Carrying the identity WITH the text — assigned in
    /// the same statement, so the two can never disagree — keeps that guarantee
    /// and lets a module's line be live.
    outgoing_module: Option<Module>,
    shown_module: Option<Module>,
    /// Progress 0 (`from`/`outgoing`) → 1 (measured width / `shown`).
    t: f32,
    last: Option<std::time::Instant>,
    frame_pending: bool,
}

impl Default for TitleMeta {
    fn default() -> Self {
        // Settled: an empty bar starts at its true width, not mid-morph.
        Self {
            from: 0.0,
            outgoing: String::new(),
            shown: String::new(),
            outgoing_module: None,
            shown_module: None,
            t: 1.0,
            last: None,
            frame_pending: false,
        }
    }
}

/// Animation slot for a mode-toggle pill (`None` for window/clock/close —
/// close is a resting pill, always visible beside the window name).
fn ctrl_index(id: PillId) -> Option<usize> {
    match id {
        PillId::Pseudo => Some(0),
        PillId::Float => Some(1),
        PillId::Fullscreen => Some(2),
        _ => None,
    }
}

/// Back-to-front draw order so each parent pill occludes the control emerging
/// from behind it: fullscreen ← pseudo ← close. (Window and clock are
/// independent resting pills — they never overlap the emerge chain.)
fn draw_z(id: PillId) -> u8 {
    match id {
        // A doorway is drawn under its sticky partner, like the control it is
        // standing in for would be.
        PillId::Doorway => 1,
        PillId::Fullscreen => 1,
        PillId::Float => 2,
        PillId::Pseudo => 3,
        // A resting pill like the close, and like it a parent: the tucked mode
        // toggles emerge from behind it, so it has to draw over them.
        PillId::Close | PillId::WindowState => 4,
        PillId::Window => 5,
        // Nested inside the window pill, so they must draw over it.
        PillId::SunsetTurnOn => 6,
        PillId::ModuleSettings => 6,
        PillId::Clock => 6,
        // The preview/box (Notif) draws first; the fixed bell (NotifMute) draws
        // on top of it, capping its right end as it grows out from behind.
        PillId::Notif => 7,
        PillId::NotifMute => 8,
        // Mirror of the bell on the left edge: the box + copy-link pill draw
        // first (emerging from behind), then the small fixed glyph pill on top.
        PillId::ClipboardBox => 9,
        PillId::ClipCopyLink => 9,
        PillId::Clipboard => 10,
        // The readout slides out from BEHIND the gear and over the clipboard
        // cluster while it is out, so it draws above that cluster and below the
        // gear itself — the same stacking the clipboard box uses under its own
        // glyph pill, one edge of the bar mirroring the other.
        PillId::SettingsStats => 11,
        // The gear is never covered: it is the one pill that must always be
        // findable, and its child emerges from under it.
        PillId::Settings => 12,
        // Dynamic OPTION controls sit in the free left-centre band, overlapping
        // nothing — drawn first (lowest z).
        PillId::Option(_) | PillId::StageMode => 0,
        // The cava children emerge from BEHIND the spectrum, so they draw
        // first and it occludes them — the same stacking the clipboard's box
        // uses under its glyph pill. The three transport symbols stay at the
        // children's level and rely on insertion order (the sort is stable) to
        // land on top of the now-playing pill they live inside.
        PillId::CavaNow | PillId::CavaOut => 0,
        PillId::Cava => 1,
        // The transport is drawn ON the spectrum pill — it is what the bars
        // turn into — so it sits above it.
        PillId::CavaPlay | PillId::CavaPrev | PillId::CavaNext => 2,
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
    /// The STAGE deck strip on the bottom edge (see [`crate::deck`]).
    Deck,
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
    /// The settings gear at the bar's very left end (Max, 2026-09-13). The
    /// first thing on the banner, and the only pill pinned to the edge itself:
    /// everything else on that side lays out to its right.
    Settings,
    /// The gear's hover child: CPU / RAM / DISK / BATTERY, sliding out from
    /// behind it exactly as the clipboard's preview does (see [`crate::stats`]).
    /// Listed as a pill so hovering it holds itself open — crossing from the
    /// gear onto the readout is transit, not departure.
    SettingsStats,
    /// The clipboard OPTION's small fixed pill: a clipboard glyph, a gap right
    /// of the settings gear. The preview/history box slides out to its right
    /// from behind it — the left-side mirror of the bell + its box (see
    /// [`crate::clipboard`]).
    Clipboard,
    /// The clipboard OPTION's morphing preview/history box (rests behind the
    /// small pill, slides out rightward). Mirrors `Notif`.
    ClipboardBox,
    /// The "copy link" pill that slides out from behind the small clipboard pill
    /// when the focused app is a browser; a click copies its current page URL.
    ClipCopyLink,
    Window,
    /// The sunset prompt's [turn on] pill, nested inside the window pill's
    /// right end while the module is asking (see "Sunset prompt" above).
    SunsetTurnOn,
    /// The sunset prompt's settings gear, a circular pill at the module's
    /// right end, right of [turn on].
    ModuleSettings,
    Close,
    /// What the focused window IS — floating, pseudo, fullscreen — as that
    /// mode's own glyph, right of the close. Nothing at all when the window is
    /// plainly tiled: the layout is the resting state of the desktop and needs
    /// no marking, so the bar only speaks up when a window has left it.
    ///
    /// It is the control for the mode it names as well as the label: clicking
    /// it returns the window to the layout, which is what pressing the mode you
    /// are already in has always done. The other two modes stay tucked behind
    /// it until the cluster is hovered.
    WindowState,
    Pseudo,
    /// Out of the layout, free-floating — the fourth window mode.
    Float,
    Fullscreen,
    /// The doorway of a sticky OPTION (`OptionUXRules.md` §4): an empty pill
    /// standing in the slot its successor will occupy. Not an action — hovering
    /// it brings the whole bar back, and the real OPTION appears in this exact
    /// place, so it reads as that pill arriving rather than as a swap.
    Doorway,
    /// A dynamic OPTION control from the Mind (media/git/call/…), identified by
    /// its index into [`crate::App::surfaced_options`]. Clicking it runs that
    /// affordance's action.
    Option(u8),
    /// The STAGE's mode switch: one task alone on the stage, or a whole desk.
    /// Only on the bar while the stage owns the screen — it is a control for the
    /// mode, and there is nothing to say about it when the mode is down.
    StageMode,
    /// The cava pill: a small bar spectrum of whatever is coming out of the
    /// speakers. Not an affordance — it is the bar SHOWING the sound rather
    /// than labelling it, which is pillar 5 taken literally.
    Cava,
    /// The cava pill's children, revealed to its right on hover. They emerge
    /// from behind it, so the spectrum is the parent and these are what it
    /// turns out to have been about all along — the second base flow, options
    /// begetting options.
    CavaPlay,
    CavaPrev,
    CavaNext,
    /// What is playing, in words.
    CavaNow,
    /// Where it is coming out.
    CavaOut,
}

impl PillId {
    /// Whether this pill belongs to the cava cluster — the parent or any child.
    /// Hovering ANY of them holds the whole cluster open, so crossing the gap
    /// between the spectrum and its controls is transit, not departure.
    /// The transport pill itself, or one of the three symbols drawn on it —
    /// the one surface the cava gestures answer to. Scrolling the track name or
    /// the output does nothing: the buttons are unmistakably the controls, so
    /// the gesture lives where the controls are and nowhere else.
    pub(crate) fn is_cava_transport(self) -> bool {
        matches!(
            self,
            PillId::Cava | PillId::CavaPlay | PillId::CavaPrev | PillId::CavaNext
        )
    }

    pub(crate) fn is_cava(self) -> bool {
        matches!(
            self,
            PillId::Cava
                | PillId::CavaPlay
                | PillId::CavaPrev
                | PillId::CavaNext
                | PillId::CavaNow
                | PillId::CavaOut
        )
    }
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
    /// While the STAGE owns it. A near-copy of `desktop` today — the bar keeps
    /// doing its job over a staged task — but it is its own situation because
    /// the stage has controls the desktop has no use for, and the day one of
    /// the desktop's pills stops making sense there, this is the line that says
    /// so rather than an `if` somewhere downstream.
    stage: bool,
}

/// Present everywhere.
const BOTH: Presence = Presence {
    desktop: true,
    overview: true,
    stage: true,
};
/// Everywhere a window is being worked in — the desktop and the stage, but not
/// the overview's map.
const DESKTOP_ONLY: Presence = Presence {
    desktop: true,
    overview: false,
    stage: true,
};
/// Only while the stage owns the screen.
const STAGE_ONLY: Presence = Presence {
    desktop: false,
    overview: false,
    stage: true,
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

/// The height EVERY open box on this bar grows to — the clipboard's history,
/// the notification drawer, the settings readout's panel. **One drawer, one
/// height** (Max, 2026-09-13: *"gear, clipboard and notis should be the same
/// height"*); they used to fit each to its own content, so three boxes that are
/// the same object in three places opened to three different sizes.
///
/// It is also the tallest that FITS: the OPTIONS surface is fixed at
/// `options.height + OPTIONS_OVERHANG + OPTIONS_DROPDOWN_H`, and a box past that
/// edge is not drawn taller — it is cut off, which is how the settings panel
/// lost its bottom corners. Raise [`crate::OPTIONS_DROPDOWN_H`] first if this
/// ever needs to grow.
pub(crate) const BOX_DRAWER_H: f32 = 505.0;

/// Zebra striping for a box's history list — alternate rows get a lightness
/// shift so adjacent lines read as distinct (old-Finder style). Direction is
/// **adaptive**: a dark box lightens its stripes, a light box darkens them,
/// keyed off the box's own luminance. The shift is in HSL **lightness only**
/// (hue/saturation untouched, see [`App::zebra_stripe`]) — units are sRGB
/// `L` (0..1, perceptual), not the linear alpha the old wash-based version
/// used, so these aren't directly comparable to a "wash alpha" intuition.
/// Symmetric (unlike the old asymmetric wash alphas): HSL's `L` is already
/// roughly perceptually uniform, so the gamma-driven asymmetry that white
/// washes needed doesn't apply here.
const STRIPE_LIFT_L: f32 = 0.18;
const STRIPE_DIM_L: f32 = 0.18;
/// Resting text opacity of an open box's list lines; the hovered line spends
/// the headroom these leave (see [`hover_ink_for`]).
///
/// They differ a lot, and the reason is the CONTRAST CEILING of each regime,
/// measured 2026-08-31: light ink on a dark box reaches ~7:1 easily, so it
/// can rest well under full (0.67 still measures 6:1) and leave a wide gap
/// for hover; dark ink on a backdrop-coloured light box tops out around
/// 5.5:1, so it has to rest near full (0.88 ≈ 3.6:1) and the hover step is
/// necessarily smaller. Muting the light-box text as far as the dark-box
/// text is what made the content unreadable earlier in the day. See
/// [`App::dim_ink`].
const LIST_DIM: f32 = 0.67;
const LIST_DIM_LIGHT: f32 = 0.88;

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

// OPTIONS' ink: REAL black and white (Max, 2026-09-12). The warm, softened
// pair it replaces (off-white #E8E5DE / near-black #262220, 2026-08-31) was
// meant to keep the text in the room's light, but it only ever cost contrast
// — the pure pair is what reads. LINEAR values, and here they are also the
// sRGB ones: 0 and 1 are the two points the transfer curve fixes.
/// Pure white, #FFFFFF.
const INK_LIGHT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// Pure black, #000000.
const INK_DARK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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

/// A colour-matchable surface's live backdrop — a flush-window match, or a
/// sampled frost fallback — and the adaptive wash/ink it implies. Shared by
/// the OPTIONS bar and the dock, the only two surfaces that colour-match a
/// window against the Hyprland layout (see [`crate::screencopy`]): "is this
/// bright / what wash / what ink" is computed by ONE formula here instead of
/// two hand-copied method families that could quietly drift apart — which is
/// exactly what the bar's and the dock's each used to be.
struct Backdrop {
    /// The flush window's colour, when one is matched.
    matched: Option<[f32; 4]>,
    /// The sampled frosted backdrop, when nothing is matched.
    frost: Option<[f32; 4]>,
}

impl Backdrop {
    /// What this surface actually sits on right now: the match if there is
    /// one, else the frost. `None` only before the first sample lands.
    fn get(&self) -> Option<[f32; 4]> {
        self.matched.or(self.frost)
    }

    /// Bright enough to want dark ink/washes. MATCHED colour only, on
    /// purpose: this drives the resting wash (and the box fill it feeds),
    /// i.e. how the surface *looks* — teaching it the frost too would
    /// restyle everything over any light wallpaper, which historically was
    /// not the contrast problem (only the ink needed to read the frost —
    /// see `App::options_text_color`, which measures `get()` directly).
    fn is_bright(&self) -> bool {
        self.matched.is_some_and(|c| luminance(c) > 0.179)
    }

    /// Resting wash — asymmetric alphas because a white wash reads
    /// stronger than a black one at equal alpha.
    fn rest_wash(&self) -> [f32; 4] {
        if self.is_bright() {
            wash(false, 0.10)
        } else {
            wash(true, 0.11)
        }
    }

    /// Hover wash — the stronger sibling of `rest_wash`, same asymmetry.
    fn hover_wash(&self) -> [f32; 4] {
        if self.is_bright() {
            wash(false, 0.30)
        } else {
            wash(true, 0.27)
        }
    }

    /// The surface's fill + the ink that reads on it: the backdrop with the
    /// resting wash composited over it ("the surface is the pill grown"),
    /// ink measured against that same washed result so the two can never
    /// disagree. `(fallback_fill, fallback_ink)` covers the brief window
    /// before the first sample lands — each caller's own native look
    /// (an OPTIONS box's neutral slab, the dock's static theme colour).
    ///
    /// `opaque = false` re-takes the alpha from `fallback_fill` instead of
    /// forcing it to `1.0`: the dock needs this (its Hyprland layer rule
    /// blurs through real transparency — forcing it opaque would silently
    /// kill the glass look); an OPTIONS box wants `true` (translucency is
    /// layered on separately, at the box's own PANEL alpha).
    fn surface(
        &self,
        fallback_fill: [f32; 4],
        fallback_ink: [f32; 4],
        opaque: bool,
    ) -> ([f32; 4], [f32; 4]) {
        match self.get() {
            Some(backdrop) => {
                let mut fill = box_fill(backdrop, self.rest_wash());
                let ink = ink_on(fill);
                if !opaque {
                    fill[3] = fallback_fill[3];
                }
                (fill, ink)
            }
            None => (fallback_fill, fallback_ink),
        }
    }
}

/// Drop every pill an open element's `rect` spans — except the ones `keep`
/// names, which are that element's own parts (they are drawn BY it, not under
/// it). Horizontal only: everything here shares the bar's one band.
///
/// The one mechanism for "something grew over the bar"; see the call site in
/// [`App::options_pills`] for why a covered pill has to leave rather than fade.
///
/// **The clock is never removed, by anything.** It is the bar's furniture,
/// pinned at the edge and present in every arrangement: it covers, it is not
/// covered. Without that, hovering the bell and then sliding right onto the
/// clock made the clock VANISH (Max, 2026-09-13) — the peeked preview's rule
/// fired on the growing date, each of the two elements claiming the ground the
/// other was standing on. Spelled here rather than in every `keep` so the next
/// element that grows over the bar cannot forget it.
fn clear_under(pills: &mut Vec<Pill>, rect: Rect, keep: fn(PillId) -> bool) {
    pills.retain(|p| {
        p.id == PillId::Clock
            || keep(p.id)
            || p.rect.x >= rect.x + rect.w
            || p.rect.x + p.rect.w <= rect.x
    });
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
        PillId::Pseudo | PillId::Float | PillId::Fullscreen => DESKTOP_ONLY,
        // The state pill says what the window IS — and on the stage every task
        // is maximized, so it would read "fullscreen" for all of them, all the
        // time. The stage is the state there; the word would only be noise.
        PillId::WindowState => Presence {
            desktop: true,
            overview: false,
            stage: false,
        },
        // The clipboard serves the window you're working in, not the map.
        PillId::Clipboard | PillId::ClipboardBox | PillId::ClipCopyLink => DESKTOP_ONLY,
        // The gear keeps its edge in every arrangement: settings are about the
        // shell itself, so they are not a thing the overview or the stage
        // replaces (and its place cannot drift, which is the point of it).
        PillId::Settings | PillId::SettingsStats => Presence {
            desktop: true,
            overview: true,
            stage: true,
        },
        // Context controls act on the focused app — meaningless over the map.
        PillId::Option(_) => DESKTOP_ONLY,
        // A control for the mode, present exactly as long as the mode is.
        PillId::StageMode => STAGE_ONLY,
        // The overview shows window management, not what you were doing —
        // and a spectrum is very much what you were doing (Max, 2026-09-12).
        PillId::Cava
        | PillId::CavaPlay
        | PillId::CavaPrev
        | PillId::CavaNext
        | PillId::CavaNow
        | PillId::CavaOut => DESKTOP_ONLY,
        // The sunset question waits for the desktop — above the overview the
        // window pill has its labelling job.
        PillId::SunsetTurnOn | PillId::ModuleSettings => DESKTOP_ONLY,
        // Only ever present while a sticky OPTION is standing, which cannot
        // happen above the overview (it has its own strip and never conceals).
        PillId::Doorway => DESKTOP_ONLY,
    }
}

/// The toggle that stands for a mode — the pill the state glyph replaces while
/// the window is in it, and where that glyph comes from.
///
/// `None` for tiled: the desktop's resting state, which every window is in
/// unless it has been taken out of it, so marking it would put a sign on the bar
/// that is true almost always and therefore says nothing. The three that are
/// worth showing are the three a window had to be *put* into.
fn mode_pill(mode: hypr::WindowMode) -> Option<PillId> {
    match mode {
        hypr::WindowMode::Tiled => None,
        hypr::WindowMode::Floating => Some(PillId::Float),
        hypr::WindowMode::Pseudo => Some(PillId::Pseudo),
        hypr::WindowMode::Fullscreen => Some(PillId::Fullscreen),
    }
}

/// The glyph a fixed control pill wears. Only the window-mode controls can go
/// sticky today (they are the ones whose action can take the bar away), so the
/// rest fall back to the generic OPTION mark rather than inventing one.
fn glyph_for_pill(id: PillId) -> &'static str {
    match id {
        PillId::Fullscreen => GLYPH_FULL,
        PillId::Pseudo => GLYPH_SQUARE,
        PillId::Float => GLYPH_FLOAT,
        PillId::Close => GLYPH_CLOSE,
        _ => GLYPH_OPTION,
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

/// Inverse of [`srgb_to_linear`]: linear (shader) → sRGB (perceptual 0..1).
/// Needed to do the zebra stripe's lightness shift (see [`App::zebra_stripe`])
/// in the space HSL actually means "lightness" in — shifting L in *linear*
/// RGB reads wrong (linear is not perceptually uniform), so the fill is
/// brought to sRGB, shifted there, then converted back for the shader.
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// RGB (any consistent space, here sRGB 0..1) → HSL. Standard formula.
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l); // achromatic — hue is undefined, 0 is fine
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let t = if t < 0.0 {
        t + 1.0
    } else if t > 1.0 {
        t - 1.0
    } else {
        t
    };
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// HSL → RGB (sRGB 0..1). Standard formula, the inverse of [`rgb_to_hsl`].
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s.abs() < 1e-6 {
        return (l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    (
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    )
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

/// Whether a `Daemon(tag)` action is one the dispatch in
/// `run_affordance_action` actually handles. This is the daemon's half of the
/// stringly-typed cross-crate tag contract (see the note on
/// `run_affordance_action`): the engine-emission coverage test asserts every
/// tag the engine can produce satisfies this, and the dispatch's fallback arm
/// `debug_assert`s the inverse, so the two lists cannot drift silently.
pub(crate) fn daemon_tag_known(tag: &str) -> bool {
    matches!(
        tag,
        "toggle_dnd"
            | "eye_protection_on"
            | "find_in_page"
            | "reopen_tab"
            | "slide_next"
            | "slide_prev"
            | "present"
            | "page_next"
            | "page_prev"
            | "undo"
            | "empty_trash"
    ) || tag.starts_with("define:")
        || tag.starts_with("pkgsearch:")
}

/// Estimated shaped width of `text` at `font_px`, char-class aware and
/// deliberately generous: the tooltip label wraps (invisibly) past its box,
/// so an UNDERestimate silently truncates the text while an overestimate
/// only widens the pill a little. Classes are em-fractions for a humanist
/// sans; +8% margin on top.
fn est_text_w(text: &str, font_px: f32) -> f32 {
    let units: f32 = text
        .chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | '!' | '|' | '\'' | '.' | ',' | ':' | ';' => 0.30,
            ' ' | '(' | ')' | '[' | ']' | '-' | 'f' | 't' | 'r' => 0.40,
            'm' | 'w' => 0.85,
            'M' | 'W' => 0.95,
            'A'..='Z' | '0'..='9' => 0.70,
            'a'..='z' => 0.55,
            _ => 1.0, // wide/unknown scripts: assume a full em
        })
        .sum();
    units * font_px * 1.08
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

    /// The pills at their RESTING positions — the layout with no leader held,
    /// i.e. exactly the layout that was always there. [`Self::options_pills`]
    /// is this plus the Leader's displacement; everything downstream reads
    /// that one.
    fn options_pills_resting(&self) -> Vec<Pill> {
        let w = self.options_size.0 as f32;
        let bar_h = self.options_bar_h();
        if w == 0.0 {
            return Vec::new();
        }
        // A sticky OPTION IS the layout while it stands (`OptionUXRules.md`
        // §4): the bar is concealed, and all that remains is the control that
        // took it away plus its doorway. Returning them here rather than
        // painting them specially means they get the surface's real drawing,
        // hit-testing and hover for free — they are ordinary pills, there just
        // aren't any others.
        if let Some(st) = self.options_sticky.as_ref() {
            return vec![
                Pill {
                    id: PillId::Doorway,
                    rect: st.door,
                    text: String::new(),
                    family: Some(NERD),
                    glyph_color: None,
                },
                Pill {
                    id: st.id,
                    rect: st.rect,
                    text: glyph_for_pill(st.id).to_owned(),
                    family: Some(NERD),
                    glyph_color: None,
                },
            ];
        }
        // Pills fill almost the whole bar height, top to bottom.
        let ph = (bar_h - 2.0 * PILL_MARGIN_Y).max(1.0);
        let y = PILL_MARGIN_Y;
        let mut pills = Vec::new();

        // Clock, far right. Its width metamorphoses between the HH:MM and the
        // full date as the pill is hovered; it grows leftward (right edge
        // pinned at the bar edge). The wider rect stays hoverable-as-clock, so
        // the date holds open while the pointer is over it.
        let clock_left = self.options_clock_left();
        if !self.options_clock.is_empty() {
            pills.push(Pill {
                id: PillId::Clock,
                rect: Rect::new(clock_left, y, w - EDGE_PAD - clock_left, ph),
                text: self.options_clock.clone(),
                family: TEXT_FONT,
                glyph_color: None,
            });
        }

        // The notification cluster hangs off the clock's RESTING edge, not its
        // live one — one anchor, spelled once ([`Self::options_clock_rest_left`])
        // — so the date growing does not drag this whole end of the bar sideways.
        // It grows over the bell instead, and the bell steps off while covered
        // (`clear_under` in `options_pills`). This block used to recompute the
        // clock's edge inline, which is how the bell stayed pinned to the LIVE
        // width even after `notif_rect` was moved onto the resting one: two
        // spellings of one anchor, disagreeing.
        let notif_right = self.options_clock_rest_left() - OPTION_GAP;

        // Notification OPTION: one element (bell → preview pill → history
        // rectangle) just left of the clock; its full drawing + morph is in
        // `crate::notif`. Its rect is the whole morphing shape (it grows *down*
        // past the bar when expanded).
        pills.push(Pill {
            id: PillId::Notif,
            rect: self.notif_geom(notif_right, y, ph),
            text: GLYPH_BELL.to_owned(),
            family: Some(NERD),
            glyph_color: None,
        });

        // Mute pill: a fixed circle in the bell's *original* resting slot. At
        // rest the bell sits exactly on top of it; as the bell peeks open it
        // slides left (see `notif_geom`) and uncovers this pill to its right.
        // Drawn under the bell (see `draw_z`).
        pills.push(Pill {
            id: PillId::NotifMute,
            rect: Rect::new(notif_right - ph, y, ph, ph),
            text: GLYPH_BELL_SLASH.to_owned(),
            family: Some(NERD),
            glyph_color: None,
        });

        // The settings gear: the very first thing on the banner, pinned to the
        // left edge (Max, 2026-09-13). Everything else on this side starts from
        // `left_start` below, so the gear's place is the one that never moves.
        //
        // Its stats child goes FIRST so the gear draws over the end it emerges
        // from; while it rests it is exactly the gear's circle, hidden behind it
        // (`stats_geom`), and it is only listed while it is actually out so it
        // cannot eat hovers meant for the gear.
        if self.stats_out() {
            pills.push(Pill {
                id: PillId::SettingsStats,
                rect: self.stats_geom(),
                text: String::new(),
                family: None,
                glyph_color: None,
            });
        }
        pills.push(Pill {
            id: PillId::Settings,
            rect: Rect::new(EDGE_PAD, y, ph, ph),
            text: GLYPH_GEAR.to_owned(),
            family: Some(NERD),
            glyph_color: None,
        });
        let left_start = self.options_left_start();

        // Clipboard OPTION: the left-edge mirror of the notification cluster.
        // A small fixed glyph pill sits just right of the gear; the preview/
        // history box slides out to its RIGHT from behind it (drawn first,
        // capped by the small pill on top). `crate::clipboard` draws the box.
        //
        // Like everything else on this edge it simply leaves while the gear's
        // readout is over it — decided once, on the finished layout, in
        // `options_pills`.
        {
            pills.push(Pill {
                id: PillId::ClipboardBox,
                rect: self.clip_geom(left_start, y, ph),
                text: String::new(),
                family: None,
                glyph_color: None,
            });
            pills.push(Pill {
                id: PillId::Clipboard,
                rect: Rect::new(left_start, y, ph, ph),
                text: GLYPH_CLIPBOARD.to_owned(),
                family: Some(NERD),
                glyph_color: None,
            });

            // Copy-link pill: slides out from behind the small clipboard pill to
            // its right when the focused app is a browser (has a copyable URL).
            let lt = self.clip_link_t();
            if lt > 0.01 {
                let out_x = left_start + ph + crate::clipboard::LINK_GAP;
                pills.push(Pill {
                    id: PillId::ClipCopyLink,
                    rect: Rect::new(lerp(left_start, out_x, lt), y, ph, ph),
                    text: GLYPH_COPY_LINK.to_owned(),
                    family: Some(NERD),
                    glyph_color: None,
                });
            }
        }

        // Dynamic OPTION pills: the Mind's context-aware controls (media
        // transport/volume/brightness, git commit/push, mute mic, open link).
        // Small glyph circles in the free band just right of the clipboard
        // cluster, in the mind's ranked order. Each carries its index into
        // `surfaced_options()`, which the click handler dispatches.
        {
            // Clear the clipboard cluster by exactly the gap the notification
            // cluster keeps from the clock — `OPTION_GAP`, the bar's one
            // between-OPTIONS distance (Max, 2026-09-12).
            //
            // At REST the cluster is one pill wide: the copy-link lives behind
            // the clipboard glyph and only slides out for a browser with a
            // copyable URL. Reserving its slot permanently — which is what this
            // used to do — pushed the band a whole pill-width further out all
            // day to make room for something usually not there.
            //
            // So the reservation rides the link's own slide factor instead of
            // being constant. The band travels with the pill that displaces it,
            // on the animation already happening, rather than jumping when it
            // appears.
            let cluster_start = left_start
                + ph
                + OPTION_GAP
                + self.clip_link_t().clamp(0.0, 1.0) * (ph + crate::clipboard::LINK_GAP);
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
            // The stage's mode switch comes before everything else in the band.
            // While the mode is up it is the biggest thing the bar can say
            // about the screen — what the screen IS right now — and it is the
            // only pill here that is about the whole screen rather than about
            // something on it.
            if self.stage.is_on() {
                pills.push(Pill {
                    id: PillId::StageMode,
                    rect: Rect::new(ox, y, ph, ph),
                    // The glyph is the mode you are IN, not the one a click
                    // would give you: the bar states, and the click changes what
                    // it states. (A pill showing the other mode reads as a
                    // label for what you are looking at and gets it backwards.)
                    text: match self.stage.mode() {
                        crate::stage::Mode::Task => GLYPH_ONE_TASK,
                        crate::stage::Mode::Desk => GLYPH_WHOLE_DESK,
                    }
                    .to_owned(),
                    family: Some(NERD),
                    glyph_color: None,
                });
                ox += ph + CTRL_GAP;
            }
            // The cava pill leads the band: it is the one thing here that is
            // not a control, and it is about the sound rather than about any
            // window, so it sits before whatever the Mind is offering.
            if self.cava_visible() {
                let cava_w = ph * CAVA_PILL_W;
                let cava_rect = Rect::new(ox, y, cava_w, ph);
                // The bars THEMSELVES become the transport on hover (Max,
                // 2026-09-12) — one pill metamorphosing in place, the way the
                // clock becomes the date, rather than three more pills arriving
                // beside it. The pill does not change size or position doing
                // it, so nothing on the bar moves: only what it is made of
                // changes.
                //
                // Pushed BEFORE the parent so the hit-test (which walks in
                // insertion order) finds a symbol rather than the pill it sits
                // on; `draw_z` puts them back on top for drawing.
                let sym_a = self.cava_sym_alpha();
                if sym_a > 0.01 {
                    let sym_span = 3.0 * CAVA_SYM_W + 2.0 * CAVA_SYM_GAP;
                    let play = if self.cava_is_playing() {
                        GLYPH_PAUSE
                    } else {
                        GLYPH_PLAY
                    };
                    let mut sx = ox + (cava_w - sym_span) / 2.0;
                    for (id, glyph) in [
                        (PillId::CavaPrev, GLYPH_PREV),
                        (PillId::CavaPlay, play),
                        (PillId::CavaNext, GLYPH_NEXT),
                    ] {
                        pills.push(Pill {
                            id,
                            rect: Rect::new(sx, y, CAVA_SYM_W, ph),
                            text: glyph.to_owned(),
                            family: Some(NERD),
                            glyph_color: None,
                        });
                        sx += CAVA_SYM_W + CAVA_SYM_GAP;
                    }
                }
                pills.push(Pill {
                    id: PillId::Cava,
                    rect: cava_rect,
                    text: String::new(),
                    family: None,
                    glyph_color: None,
                });
                // The children, revealed to the right on hover. Each emerges
                // from BEHIND the spectrum — at t=0 every one of them sits
                // exactly on the parent's rect and is invisible, so the cluster
                // costs nothing when unused (§4's "stickiness is a grace").
                //
                // They are laid out left to right in the order Max asked for:
                // play/pause, previous, next, what is playing, where it goes.
                let t = self.cava_reveal;
                if t > 0.01 {
                    let mut cx = ox + cava_w + OPTION_GAP;
                    // The transport lives INSIDE the now-playing pill as bare
                    // symbols, not as three pills of its own (Max, 2026-09-12).
                    // One object — "this is playing, and here is how you drive
                    // it" — rather than four things in a row that happen to be
                    // adjacent.
                    // A child is WIDER than the spectrum it comes out of, so no
                    // x-position can hide it: it has to grow as well as slide.
                    // At t=0 it is a zero-width sliver sitting exactly on the
                    // parent's right edge — invisible, and behind it besides —
                    // and it opens rightward into its slot as t rises. That is
                    // the second base flow, options begetting options.
                    let emerge = |t: f32, slot_x: f32, full_w: f32| -> Rect {
                        let edge = ox + cava_w;
                        Rect::new(lerp(edge, slot_x, t), y, full_w * t, ph)
                    };
                    let now = self.cava_now_text();
                    if !now.is_empty() {
                        // Just words now: the transport went home to the bars.
                        // Sized from the EASED width, so a track change resizes
                        // the pill smoothly instead of snapping to the new
                        // song's length.
                        // Room for the trailing time too, or the title's column
                        // would be squeezed by something the width never
                        // accounted for.
                        let time = if self.cava_time_w > 0.0 {
                            self.cava_time_w + CAVA_TIME_GAP
                        } else {
                            0.0
                        };
                        let w =
                            (2.0 * PILL_PAD_X + self.cava_now_w_eased() + time + CAVA_NOW_EXTRA)
                                .clamp(ph, ph * CAVA_NOW_MAX_W);
                        let mut grown = emerge(t, cx, w);
                        // …and then DOWN into the list, the way the clipboard
                        // pill grows into its history. One rect: the thing you
                        // opened is the thing you are looking at.
                        grown.h = lerp(ph, self.play_box_full_h(), self.play_box_e);
                        pills.push(Pill {
                            id: PillId::CavaNow,
                            rect: grown,
                            text: now,
                            family: TEXT_FONT,
                            glyph_color: None,
                        });
                        cx += w + CTRL_GAP;
                    }
                    // Where the sound comes out stays its own pill: it is about
                    // the device, not about the track.
                    let out = self.cava_out_text();
                    if !out.is_empty() {
                        // A circle carrying the volume at rest, opening into
                        // `[95%][device]` on hover. The volume's slot never
                        // moves or resizes; only what follows it grows.
                        let slot = self.cava_vol_slot();
                        let full = slot + self.cava_out_w + PILL_PAD_X;
                        let w = lerp(slot, full.max(slot), self.cava_out_t);
                        pills.push(Pill {
                            id: PillId::CavaOut,
                            rect: emerge(t, cx, w),
                            text: out,
                            family: TEXT_FONT,
                            glyph_color: None,
                        });
                        cx += w + CTRL_GAP;
                    }
                    ox = cx - CTRL_GAP + OPTION_GAP;
                } else {
                    ox += cava_w + OPTION_GAP;
                }
            }
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
        }

        // The window name pill is centred *alone* (so it doesn't shift when the
        // toggles reveal); close rests beside it, ALWAYS visible; the mode
        // toggles hide until hover, at fixed resting spots right of the close:
        //   [window name] [X] [pseudo] [fullscreen]
        // A module stands the pill up even with no window to name — which is the
        // whole point on an empty workspace, where the cluster would otherwise be
        // absent (Max, 2026-09-08, of the sunset offer: "it should pop up even on
        // empty spaces").
        if self.options_title.is_some() || self.overview_hover.is_some() || self.module_on_pill() {
            // Title, with the live resize readout appended while active.
            let shown = self.options_window_text().unwrap_or_default();
            // Morphing width, not the raw measurement: the pill eases between
            // two titles instead of jumping (see [`TitleMeta`]).
            let ww = (self.options_title_content_w() + 2.0 * PILL_PAD_X).max(ph);
            let d = ph; // control-circle diameter
            let wx = ((w - ww) / 2.0).max(EDGE_PAD);
            // The module's nested children: sunset's [turn on], and the settings
            // gear every module carries. Listed BEFORE the window pill so the
            // overlap hover resolves to them, and kept through the back-morph
            // (alpha, not presence-flag, decides) so they fade out with the
            // sentence they belong to.
            let ta = self.module_child_alpha();
            if ta > 0.01 {
                // Single source of truth (also read by the message's clip in
                // the draw pass below — see `module_nested_rects`).
                let (turn_on, gear) = self.module_nested_rects();
                if let Some(turn_on) = turn_on {
                    pills.push(Pill {
                        id: PillId::SunsetTurnOn,
                        rect: turn_on,
                        text: SUNSET_TURN_ON_LABEL.to_owned(),
                        family: TEXT_FONT,
                        glyph_color: None,
                    });
                }
                if self.module_drawn().is_some_and(Module::has_gear) {
                    pills.push(Pill {
                        id: PillId::ModuleSettings,
                        // Gear when closed; an × once the box is (mostly) open,
                        // so it reads as the panel's close button.
                        rect: gear,
                        text: if self.module_box_e > 0.5 {
                            GLYPH_CLOSE.to_owned()
                        } else {
                            GLYPH_GEAR.to_owned()
                        },
                        family: Some(NERD),
                        glyph_color: None,
                    });
                }
            }
            let circle = |pills: &mut Vec<Pill>, x: f32, id, glyph: &str, color| {
                pills.push(Pill {
                    id,
                    rect: Rect::new(x, y, d, d),
                    text: glyph.to_owned(),
                    family: Some(NERD),
                    glyph_color: color,
                });
            };
            // The asking module's step, riding the morph so it swells and
            // settles with the question and shrinks back with the title. When
            // the gear opens the settings box the SAME rect grows downward into
            // the panel (see `module_rect`) — one shape becoming the box,
            // like the notif/clipboard OPTIONS.
            pills.push(Pill {
                id: PillId::Window,
                rect: self.module_rect(),
                text: shown,
                family: TEXT_FONT,
                glyph_color: None,
            });
            // While a module holds the pill, the window controls step aside:
            // [X] beside "do you want…?" reads as an answer to the question, and
            // the controls act on a task the pill is no longer showing. They
            // return with the title. (On an empty workspace there is nothing for
            // them to act on anyway.)
            if !self.module_on_pill() {
                // Close, right of the window name — a resting pill, no reveal.
                let close_x = wx + ww + GROUP_GAP;
                circle(&mut pills, close_x, PillId::Close, GLYPH_CLOSE, None);
                let mut cx = close_x + d + GROUP_GAP;
                // What the window IS, right of the close — its mode's own glyph,
                // and only when it is something: a tiled window is the ordinary
                // case and gets no marking (Max, 2026-09-12; the word it first
                // showed became the icon on his next pass).
                let state = mode_pill(self.options_mode_shown);
                if let Some(active) = state {
                    circle(
                        &mut pills,
                        cx,
                        PillId::WindowState,
                        glyph_for_pill(active),
                        None,
                    );
                    cx += d + CTRL_GAP;
                }
                // The window modes, ordered by how far each takes the window
                // from the layout: pseudo (still tiled, just smaller), float
                // (out of the layout), fullscreen (over everything). The one the
                // window is already in is left out — the word above IS its
                // control, and offering the same mode twice in one row would be
                // two ways to press the same thing.
                for (id, glyph) in [
                    (PillId::Pseudo, GLYPH_SQUARE),
                    (PillId::Float, GLYPH_FLOAT),
                    (PillId::Fullscreen, GLYPH_FULL),
                ] {
                    if state == Some(id) {
                        continue;
                    }
                    circle(&mut pills, cx, id, glyph, None);
                    cx += d + CTRL_GAP;
                }
            }
        }
        // The presence contract, applied once: everything downstream (draw,
        // hit-test, hover, click) reads this list, so an element that does
        // not belong in the current situation simply isn't there.
        pills.retain(|p| {
            let pres = presence(p.id);
            // One situation at a time, most specific first: the overview draws
            // over the stage if it ever came up under it, and the stage over the
            // plain desktop.
            if self.overview_active {
                pres.overview
            } else if self.stage.is_on() {
                pres.stage
            } else {
                pres.desktop
            }
        });
        pills
    }

    /// The height every open box grows to, at the live scale — see
    /// [`BOX_DRAWER_H`]. The one place the boxes agree about their size.
    pub(crate) fn options_box_drawer_h(&self) -> f32 {
        BOX_DRAWER_H * self.options_scale()
    }

    /// How far in from each edge the bar's DRAWERS can reach — the clipboard
    /// and the settings readout on the left, the notification box on the right
    /// — at their fullest, regardless of what is open right now.
    ///
    /// This is the frost sampler's definition of "ours" (see
    /// `screencopy::read_sample`): it reads the wallpaper just outside these
    /// spans, so a box wears the same colour whether it is shut or open.
    pub(crate) fn options_left_drawer_right(&self) -> f32 {
        self.clip_span_full_right()
            .max(self.stats_span_full_right())
    }

    /// The right edge's twin of [`Self::options_left_drawer_right`].
    pub(crate) fn options_right_drawer_left(&self) -> f32 {
        self.notif_span_full_left()
    }

    /// Where the bar's own pills are, for a sampler that must not read them:
    /// the frosted backdrop is sampled on a row *through the bar*, which is
    /// also the row every pill sits on (see `screencopy::read_sample`).
    pub(crate) fn options_pill_columns(&self) -> Vec<Rect> {
        self.options_pills().into_iter().map(|p| p.rect).collect()
    }

    /// The pills as DRAWN: the resting layout with the Leader's displacement
    /// applied to its group (`OptionUXRules.md` §1). Draw, hit-test, hover and
    /// click all read this, so a displaced `[X]` is still hit-tested as `[X]`.
    /// With no leader held and nothing easing home this is bit-identical to
    /// [`Self::options_pills_resting`] — at rest, nothing changed.
    fn options_pills(&self) -> Vec<Pill> {
        let mut pills = self.options_pills_resting();
        let off = self.lead_offsets(&pills);
        if !off.iter().all(|o| *o == 0.0) {
            for p in pills.iter_mut() {
                if let Some(s) = group_slot(group_of(p.id)) {
                    p.rect.x += off[s];
                }
            }
        }
        // Whatever an open element is standing over LEAVES the bar for as long
        // as it is out — decided here, on the finished layout, because it is
        // the only place that knows where every pill actually ended up.
        //
        // It has to be removal rather than a fade: the clipboard, the bell and
        // the cava cluster all draw themselves and `continue` before the
        // per-pill fade exists, and glyphs are drawn in one late pass — so a
        // covered pill's icon lands ON TOP of whatever covers it. Seen three
        // times now, all on 2026-09-13: the clipboard glyph over the gear's
        // readout, the media transport symbols straight through that readout
        // the moment something played, and the same transport symbols through
        // the CLIPBOARD's own box (Max: *"i can see 'play' under clipboard when
        // it grows"*). Every element that widens over the bar pays the same
        // rule now, so the next one cannot reintroduce it.
        // The clock goes FIRST: it grows leftward into the date and takes the
        // bell's place while it is out (Max, 2026-09-13). Ordering matters
        // because the bell's own peek claims ground too — whoever runs first
        // wins the overlap, and between those two the clock wins. It is read
        // off the finished layout rather than recomputed, so the pill that
        // covers and the pill that is drawn are the same rect.
        if self.clock_covering() {
            if let Some(clock) = pills
                .iter()
                .find(|p| p.id == PillId::Clock)
                .map(|p| p.rect)
            {
                clear_under(&mut pills, clock, |id| id == PillId::Clock);
            }
        }
        if self.stats_covering() {
            clear_under(&mut pills, self.stats_geom(), |id| {
                matches!(id, PillId::Settings | PillId::SettingsStats)
            });
        }
        if self.clip_covering() {
            clear_under(&mut pills, self.clip_rect(), |id| {
                matches!(
                    id,
                    // Its own satellites (the copy-link pill rides OUT of the
                    // box's right edge), and the gear cluster it grew out from.
                    PillId::Clipboard
                        | PillId::ClipboardBox
                        | PillId::ClipCopyLink
                        | PillId::Settings
                        | PillId::SettingsStats
                )
            });
        }
        if self.notif_covering() {
            clear_under(&mut pills, self.notif_rect(), |id| {
                matches!(id, PillId::Notif | PillId::NotifMute)
            });
        }
        pills
    }

    /// Current displacement per leadable group: the stored (easing-home) value,
    /// overridden for the held group by the live one. The held group's offset is
    /// *derived*, not stored, which is what pins the leader through a layout
    /// change that lands between two pointer events.
    fn lead_offsets(&self, resting: &[Pill]) -> [f32; LEAD_N] {
        let mut off = self.options_lead.off;
        if let Some((slot, live)) = self.live_lead(resting) {
            off[slot] = live;
        }
        off
    }

    /// Where the held group must sit for the leader to stay on its anchor,
    /// bounded by its neighbours. `None` when nothing is held or the leader has
    /// left the layout.
    fn live_lead(&self, resting: &[Pill]) -> Option<(usize, f32)> {
        let ld = self.options_leader.as_ref()?;
        let id = self.leader_pill_id()?;
        let slot = group_slot(group_of(id))?;
        let nat = resting
            .iter()
            .find(|p| p.id == id && self.ctrl_pill_visible(p.id))?;
        let raw = lead_shift(nat.rect, ld.anchor_cx);
        let g = group_of(id);
        let span = self.group_span(resting, g)?;
        let (bl, br) = self.group_barriers(resting, g, span);
        let bar_w = self.options_size.0 as f32;
        Some((
            slot,
            clamp_shift(span, bl, br, (EDGE_PAD, bar_w - EDGE_PAD), OPTION_GAP, raw),
        ))
    }

    /// The leader's pill id *now*. For a Mind control the leader is held by its
    /// affordance id, so a re-rank moves the leader to the action's new slot
    /// rather than handing a stationary finger a different action; a withdrawn
    /// action yields `None` and the leader is released.
    fn leader_pill_id(&self) -> Option<PillId> {
        let ld = self.options_leader.as_ref()?;
        match ld.action {
            Some(act) => self
                .surfaced_options()
                .iter()
                .take(OPTION_PILL_CAP)
                .position(|a| a.id == act)
                .map(|i| PillId::Option(i as u8)),
            None => Some(ld.id),
        }
    }

    /// A group's resting extent (leftmost left edge, rightmost right edge)
    /// over the pills that are actually up — a tucked toggle must not
    /// constrain a shift it isn't visible for.
    fn group_span(&self, resting: &[Pill], g: PillGroup) -> Option<(f32, f32)> {
        resting
            .iter()
            .filter(|p| group_of(p.id) == g && self.ctrl_pill_visible(p.id))
            .fold(None, |acc: Option<(f32, f32)>, p| {
                let (l, r) = (p.rect.x, p.rect.x + p.rect.w);
                Some(acc.map_or((l, r), |(al, ar)| (al.min(l), ar.max(r))))
            })
    }

    /// The nearest neighbouring edges on either side of a group — what "yields
    /// to the edges" measures against.
    fn group_barriers(
        &self,
        resting: &[Pill],
        g: PillGroup,
        span: (f32, f32),
    ) -> (Option<f32>, Option<f32>) {
        let (gl, gr) = span;
        let others = resting
            .iter()
            .filter(|p| group_of(p.id) != g && self.ctrl_pill_visible(p.id));
        let mut left: Option<f32> = None;
        let mut right: Option<f32> = None;
        for p in others {
            let (l, r) = (p.rect.x, p.rect.x + p.rect.w);
            if r <= gl {
                left = Some(left.map_or(r, |b: f32| b.max(r)));
            } else if l >= gr {
                right = Some(right.map_or(l, |b: f32| b.min(l)));
            }
        }
        (left, right)
    }

    /// The window pill's content width right now — the morphing value, easing
    /// between the outgoing title's width and the measured current one.
    pub(crate) fn options_title_content_w(&self) -> f32 {
        lerp(
            self.options_title_meta.from,
            self.options_title_w,
            self.options_title_meta.t,
        )
    }

    /// Where the bar's LEFT side lays out from: past the settings gear, at the
    /// bar's one between-OPTIONS distance.
    ///
    /// The one place that answer lives. The clipboard's box computes its own
    /// rect for hit-testing and the input region ([`Self::clip_rect`]) rather
    /// than reading the drawn pill, so an anchor spelled out twice is an
    /// invisible drift waiting for the next pill on this edge — which is exactly
    /// what the gear was.
    pub(crate) fn options_left_start(&self) -> f32 {
        EDGE_PAD + self.options_pill_h() + OPTION_GAP
    }

    /// The clock pill's current left edge — where it is RIGHT NOW, mid-date-
    /// metamorphosis included. What the clock itself is drawn from.
    pub(crate) fn options_clock_left(&self) -> f32 {
        let content_w = lerp(
            self.options_clock_w,
            self.options_date_w,
            self.options_clock_meta.t,
        );
        self.clock_left_for(content_w)
    }

    /// The clock pill's left edge at its RESTING size — the time, not the date.
    /// **This is the anchor the notification element pins its right edge a gap
    /// left of**, so the clock growing no longer drags the bell sideways: it
    /// grows OVER it, and the bell steps off the bar for as long as it is
    /// covered (Max, 2026-09-13: *"clock should cover notis when growing"*).
    /// The old live anchor meant a hover on the clock re-flowed the whole right
    /// end of the bar — ~180px of it — which is the re-flow `OptionUXRules.md`
    /// §2 exists to forbid, and is why the date's collapse needed a hold timer
    /// to keep it from happening under a travelling pointer.
    pub(crate) fn options_clock_rest_left(&self) -> f32 {
        self.clock_left_for(self.options_clock_w)
    }

    /// Shared spine of the two: the left edge of a right-pinned clock pill
    /// holding `content_w` of text, or the bar's right padding when there is no
    /// clock at all.
    fn clock_left_for(&self, content_w: f32) -> f32 {
        let w = self.options_size.0 as f32;
        if self.options_clock.is_empty() {
            return w - EDGE_PAD;
        }
        let cw = (content_w + 2.0 * PILL_PAD_X).max(self.options_pill_h());
        w - EDGE_PAD - cw
    }

    /// Whether the clock has grown past its resting edge, so what it now stands
    /// over leaves the bar (see [`clear_under`]). Measured from the geometry
    /// rather than the morph progress: `t` restarts for any clock text change —
    /// a minute ticking over is a morph too — and only the date's extra width
    /// covers anything.
    fn clock_covering(&self) -> bool {
        self.options_clock_left() + 1.0 < self.options_clock_rest_left()
    }

    /// The window pill's text: the groomed, truncated title, with the live size
    /// appended — `current task (342x343)` — while a resize is in flight.
    /// In the overview the pill follows the POINTER instead of the focused
    /// window, so a hovered thumbnail's title supersedes it.
    ///
    /// The title is groomed before it is truncated ([`crate::task_title`]): the
    /// OPTION is named *current task*, so it says the task, not the app's
    /// chrome around it — and the truncation budget is then spent on the words
    /// that matter rather than on a browser's signature.
    fn options_window_text(&self) -> Option<String> {
        // A module takes the pill over (desktop only — above the overview the
        // pill labels the hovered thumbnail, a job it keeps, and the overview
        // shows window management rather than context OPTIONS anyway). Never
        // groomed, never truncated, never size-suffixed: the sentence is its own
        // text.
        if let Some(module) = self.module_shown.filter(|_| !self.overview_active) {
            return Some(module.msg().to_string());
        }
        // A hovered thumbnail is groomed WITHOUT a class: the one class we hold
        // belongs to the focused window, and letting it testify about someone
        // else's title is how a wrong signature gets stripped.
        let (raw, class) = match self
            .overview_hover
            .as_ref()
            .filter(|_| self.overview_active)
        {
            Some(hover) => (hover, None),
            None => (self.options_title.as_ref()?, self.options_class.as_deref()),
        };
        let title = truncate(
            &crate::task_title::groom(raw, class, crate::task_title::home()),
            TITLE_MAX,
        );
        // Grooming can only ever leave the pill empty when there was nothing to
        // say — and an empty pill is no pill (the cluster stands down).
        if title.is_empty() {
            return None;
        }
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
        // The cava cluster's two text children. Measured every pass like the
        // clock: the track changes under us, so a stale width would leave the
        // pill fitting the previous song.
        let now_text = self.cava_now_text();
        let out_text = self.cava_out_text();
        let vol_text = self.cava_vol_text();
        let time_text = self.cava_time_text();
        // Pill text scales with the bar (see `options_bar_h`): measure at the
        // same scaled size the draw uses, so pill widths always fit their text.
        let font_px = FONT_PX * self.options_scale();
        // The settings gear is a circle a pill-height wide (computed before the
        // renderer borrow below).
        let gear_w = self.options_pill_h();
        let Some(r) = self.options_renderer.as_mut() else {
            return;
        };
        let cw = r.measure_text(&clock, font_px, TEXT_FONT);
        let dw = r.measure_text(&date, font_px, TEXT_FONT);
        let mut tw = title
            .as_deref()
            .map_or(0.0, |t| r.measure_text(t, font_px, TEXT_FONT));
        // A module's nested controls live INSIDE its pill, so the morph target
        // width must include them — the expansion is one ease, sentence and
        // controls arriving as one shape. The sunset module carries [turn on] AND
        // the settings gear; the empty room carries the gear alone.
        if let Some(module) = self.module_shown.filter(|_| !self.overview_active) {
            self.module_text_w = tw;
            if module.has_turn_on() {
                let inner =
                    r.measure_text(SUNSET_TURN_ON_LABEL, font_px, TEXT_FONT) + 2.0 * PILL_PAD_X;
                self.sunset_inner_w = inner;
                tw += MODULE_GAP + inner + SUNSET_INNER_GAP;
                if module.has_gear() {
                    tw += gear_w;
                }
            } else if module.has_gear() {
                tw += MODULE_GAP + gear_w;
            }
        }
        self.cava_now_w = if now_text.is_empty() {
            0.0
        } else {
            r.measure_text(&now_text, font_px, TEXT_FONT)
        };
        self.cava_out_w = if out_text.is_empty() {
            0.0
        } else {
            r.measure_text(&out_text, font_px, TEXT_FONT)
        };
        // Remember WHAT was measured, not just how wide it was. The track
        // changes under us on its own schedule, and nothing in the old trigger
        // list (window title, clock tick, resize) fires when it does — so the
        // pill kept the previous song's width and the text ran out of it.
        self.cava_vol_w = if vol_text.is_empty() {
            0.0
        } else {
            r.measure_text(&vol_text, font_px, TEXT_FONT)
        };
        self.cava_now_measured = now_text;
        self.cava_out_measured = out_text;
        self.cava_time_w = if time_text.is_empty() {
            0.0
        } else {
            r.measure_text(&time_text, font_px, TEXT_FONT)
        };
        self.cava_vol_measured = vol_text;
        self.cava_time_measured = time_text;
        self.options_clock_w = cw;
        self.options_date_w = dw;
        // The window pill EASES between title widths instead of jumping (the
        // visible half of the Leader rule). Keyed on the text, not the width,
        // so a scale change re-measures without pretending the title changed.
        let shown = title.unwrap_or_default();
        let displayed = self.options_title_content_w();
        self.options_title_w = tw;
        if shown != self.options_title_meta.shown {
            let quiet = self.title_churn(&shown);
            // The module the incoming text belongs to, swapped in beside it —
            // see `TitleMeta::shown_module`. `module_shown` is the intent
            // `options_window_text` just drew from, so the pair is consistent by
            // construction rather than by comparison.
            let incoming_module = self.module_shown.filter(|_| !self.overview_active);
            let outgoing_module =
                std::mem::replace(&mut self.options_title_meta.shown_module, incoming_module);
            self.options_title_meta.outgoing_module = outgoing_module;
            let outgoing = std::mem::replace(&mut self.options_title_meta.shown, shown);
            // Churn adopts the new words WITHOUT a metamorphosis — the pill
            // simply shows them, at their own width. Only a real change of task
            // earns the crossfade.
            //
            // It lands the morph rather than leaving one in flight. A morph
            // that keeps running toward a target that just moved can have its
            // remaining span cut to nothing, and `animation::settle_t` caps its
            // tolerance at half the progress — so it would call itself finished
            // at `t = 0.5` and freeze there, which draws the title at a tenth
            // of its opacity and never recovers. Quiet must mean settled.
            if quiet {
                self.options_title_meta.from = self.options_title_w;
                self.options_title_meta.t = 1.0;
                self.options_title_meta.outgoing.clear();
                self.options_title_meta.outgoing_module = None;
            } else {
                self.begin_title_morph(displayed, outgoing);
            }
            self.options_title_addr = self.options_active_addr.clone();
            self.options_title_at = Some(Instant::now());
        }
    }

    /// Whether this title change is the same window **retitling itself** rather
    /// than a task you moved to — the backstop against a pill that blinks.
    ///
    /// The pill crossfades on a change of text, which is right when the text is
    /// a different task and wrong when it is the same task wearing a new
    /// spinner frame, unread count or elapsed timer. Those rewrite the title on
    /// the app's own schedule, roughly once a second, and each rewrite used to
    /// restart the fade — the pill strobed while an agent worked.
    /// [`crate::task_title`] grooms most of that away at the source; this
    /// catches what grooming cannot see.
    ///
    /// The test is `OptionUXRules.md` §2's, applied to time instead of layout:
    /// a change **you** asked for always plays. So focus moving to another
    /// window is never churn, a module arriving or leaving is never churn, and an
    /// overview title is never churn (it follows your pointer, one hover at a
    /// time). What is left — one window, rewriting itself again within a breath
    /// — is the app's timer talking, and the bar stays still for it.
    fn title_churn(&self, _incoming: &str) -> bool {
        // A module arriving, leaving, or rewording its own line is never churn:
        // it speaks when it has something to say, on nobody's timer.
        if self.module_shown.is_some() || self.options_title_meta.shown_module.is_some() {
            return false;
        }
        if self.overview_active {
            return false;
        }
        let Some(addr) = self.options_active_addr.as_deref() else {
            return false;
        };
        if self.options_title_addr.as_deref() != Some(addr) {
            return false;
        }
        self.options_title_at
            .is_some_and(|at| at.elapsed() < TITLE_CHURN_GAP)
    }

    /// The bar's live colour regime — matched window, else sampled frost.
    /// The one place `options_bar_matched`/`options_pill_color` are read
    /// into a [`Backdrop`]; every adaptive-colour method below goes through
    /// this instead of touching those fields directly.
    fn options_regime(&self) -> Backdrop {
        Backdrop {
            matched: self.options_bar_matched,
            frost: self.options_pill_color,
        }
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
        self.options_regime().is_bright()
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
    ///
    /// One bar-wide value, deliberately — two attempts at anything smarter
    /// (each pill measuring its own local bucket; then one sample averaged
    /// across the whole bar instead of just beside notif) were both tried
    /// and reverted 2026-09-08/09: the per-pill version read as an
    /// inconsistent black/white patchwork, and the wide-average version
    /// still visibly flipped over time — Max: "the algorithm clearly dont
    /// know what to choose." Back to the one plain notif-adjacent sample.
    /// See memory `clip-notif-frost-bug` before attempting either direction
    /// again.
    fn options_backdrop(&self) -> Option<[f32; 4]> {
        self.options_regime().get()
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
        self.options_regime().rest_wash()
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
    /// The banner (top-bar strip) fill: the ACTUAL surface the bar paints —
    /// the matched window colour when colour-matched, the opaque slab under
    /// reduce-transparency / a paused (fullscreen) bar, else the faint 10%
    /// strip that lets the frosted wallpaper read through. Returned WITHOUT the
    /// `options_show` fade so callers apply their own presence. `hard` = the
    /// bar draws it with the crisp `glass = -1.0` cut (matched/opaque regimes).
    /// The one definition of "the banner surface", shared by the strip and by
    /// the layer drawn BEHIND the sunset module (so they are literally the same
    /// material).
    pub(crate) fn options_bar_fill(&self) -> ([f32; 4], bool) {
        match self.options_bar_matched {
            Some(c) => (c, true),
            None if self.options_paused() || self.config.accessibility.reduce_transparency => {
                (self.options_box_surface().0, true)
            }
            None => ([0.0, 0.0, 0.0, 0.10], false),
        }
    }

    /// One formula for both regimes: the backdrop (matched window colour,
    /// else the sampled wallpaper) with the pill wash over it — the box is
    /// the pill grown. NOTE: opaque. Translucency belongs to the box PANEL
    /// and its zebra only (each box applies [`BOX_ALPHA`] there); this fill
    /// is also the clip detail card, the dictionary panel and the
    /// notification icon discs, and making it translucent wholesale turned
    /// those glassy too (Max, 2026-09-01: "the clipboard big pill became
    /// transparent... i only want the boxes to be blured"). Ink is measured
    /// against the box's OWN fill, so it lands on the same answer the bar
    /// reaches for the same backdrop — the two agree by measurement rather
    /// than by one borrowing the other's decision.
    pub(crate) fn options_box_surface(&self) -> ([f32; 4], [f32; 4]) {
        let slab = [BOX_SLAB[0], BOX_SLAB[1], BOX_SLAB[2], 1.0];
        self.options_regime().surface(slab, ink_on(slab), true)
    }

    /// The clipboard box's OWN regime — same matched window (that's genuinely
    /// bar-wide, position-independent), but its OWN frosted backdrop, sampled
    /// beside `clip_rect()` (left edge) rather than `notif_rect()` (right
    /// edge). **This split is why `options_box_surface` used to be wrong for
    /// the clipboard box**: both boxes read the ONE `options_pill_color`,
    /// which is only ever sampled next to notif — so an unmatched clipboard
    /// box took whatever the wallpaper happens to be on the *opposite side of
    /// the screen*, not what's actually behind it (Max, 2026-09-08: caught
    /// live — the clipboard zebra was violet, notif's was neutral grey, off
    /// the same purple-nebula wallpaper sampled at two different x-positions).
    fn clip_regime(&self) -> Backdrop {
        Backdrop {
            matched: self.options_bar_matched,
            frost: self.clip_pill_color,
        }
    }

    /// The surface a box at `rect` should wear: the same formula every box on
    /// this bar uses, reading the frost sampled on **its own side of the
    /// screen**.
    ///
    /// The bar takes two frost readings — one beside the notification box on the
    /// right ([`Slot::BarFrost`](crate::screencopy::Slot::BarFrost)), one beside
    /// the clipboard on the left (`ClipFrost`) — because a wallpaper that
    /// changes left-to-right makes a single reading wrong for one end. Which one
    /// a box reads was, until now, decided PER MODULE: the clipboard asked for
    /// the left, everybody else took the right by default. So the settings
    /// readout's box — an inch from the clipboard, on the same edge — wore the
    /// colour of the far side of the screen, and Max saw it immediately
    /// (2026-09-13: *"why does the clipboar is bluer than the gear?"*).
    ///
    /// Deciding by POSITION instead makes that impossible to get wrong again: a
    /// box wears its own side, whatever module it belongs to and wherever it
    /// moves to. The two existing boxes resolve to exactly what they already
    /// had.
    pub(crate) fn box_surface_at(&self, rect: Rect) -> ([f32; 4], [f32; 4]) {
        let mid = self.options_size.0 as f32 / 2.0;
        let frost = if rect.x + rect.w / 2.0 < mid {
            self.clip_pill_color
        } else {
            self.options_pill_color
        };
        let slab = [BOX_SLAB[0], BOX_SLAB[1], BOX_SLAB[2], 1.0];
        Backdrop {
            matched: self.options_bar_matched,
            frost,
        }
        .surface(slab, ink_on(slab), true)
    }

    /// The clipboard box's twin of `options_box_surface` — identical formula,
    /// its OWN correctly-positioned frost (see `clip_regime`).
    pub(crate) fn clip_box_surface(&self) -> ([f32; 4], [f32; 4]) {
        let slab = [BOX_SLAB[0], BOX_SLAB[1], BOX_SLAB[2], 1.0];
        self.clip_regime().surface(slab, ink_on(slab), true)
    }

    /// Hover wash — stronger than the resting wash, with the same asymmetry.
    pub(crate) fn options_hover_wash(&self) -> [f32; 4] {
        self.options_regime().hover_wash()
    }

    /// Adaptive zebra stripe colour for a list row: lighten a dark `fill`,
    /// darken a light one, pre-composited into an OPAQUE colour (so
    /// overlapping stripe pieces overwrite instead of double-blending) at
    /// the box's own panel alpha. The one formula every striped OPTIONS box
    /// list shares (clipboard history, notification history) — it used to
    /// be hand-copied into each, with nothing to stop them drifting apart.
    ///
    /// HSL, not a wash: blending toward pure white/black (the old approach —
    /// `wash()` is achromatic by definition) desaturates every stripe toward
    /// grey regardless of the fill's actual colour, and the more contrast you
    /// ask for the greyer it gets — that IS the "always the same ugly grey"
    /// Max flagged (2026-09-08), not a tuning problem. Shifting HSL
    /// *lightness* only, at the fill's own hue and saturation, gives a
    /// stripe that reads as "one shade lighter/darker of THIS colour" — a
    /// purple box's stripe stays visibly purple, a green one stays green;
    /// only an actually-neutral fill produces a neutral stripe. Done in sRGB
    /// (`linear_to_srgb`) because HSL's L is only perceptually meaningful in
    /// a perceptual space — shifting it in linear reads wrong.
    pub(crate) fn zebra_stripe(&self, fill: [f32; 4]) -> [f32; 4] {
        let srgb = [
            linear_to_srgb(fill[0]).clamp(0.0, 1.0),
            linear_to_srgb(fill[1]).clamp(0.0, 1.0),
            linear_to_srgb(fill[2]).clamp(0.0, 1.0),
        ];
        let (h, s, l) = rgb_to_hsl(srgb[0], srgb[1], srgb[2]);
        let new_l = if luminance(fill) <= 0.179 {
            (l + STRIPE_LIFT_L).min(1.0)
        } else {
            (l - STRIPE_DIM_L).max(0.0)
        };
        let (r, g, b) = hsl_to_rgb(h, s, new_l);
        [
            srgb_to_linear(r),
            srgb_to_linear(g),
            srgb_to_linear(b),
            self.box_panel_alpha(),
        ]
    }

    /// Resting list-ink for a box's lines, dimmed by the shared
    /// [`LIST_DIM`]/[`LIST_DIM_LIGHT`] (see there for why they differ so
    /// much). `ink` must be one of the two fixed [`ink_on`] outputs
    /// (`INK_LIGHT`/`INK_DARK`) — the channel-sum check is just a cheap
    /// discriminator between those two known constants, not a real
    /// luminance read.
    pub(crate) fn dim_ink(&self, ink: [f32; 4]) -> [f32; 4] {
        let is_light_ink = ink[0] + ink[1] + ink[2] < 1.5;
        let list_dim = if is_light_ink {
            LIST_DIM_LIGHT
        } else {
            LIST_DIM
        };
        [ink[0], ink[1], ink[2], ink[3] * list_dim]
    }

    /// Add the OPTIONS pills to the bar's scene (called after the base fill).
    /// Control buttons carry the reveal animation: a horizontal slide from
    /// behind their parent pill (offset from `slide`) plus an opacity from
    /// `alpha`; window/clock are always at rest, full opacity.
    /// Discoverability: while a context OPTION pill (an icon-only glyph) is
    /// hovered, show its offer title in a small label just below the bar. Only
    /// the dynamic OPTION pills get this — the fixed pills (clock, bell,
    /// clipboard) reveal their own labels.
    /// Whether the cava pill is on the bar: something is playing.
    ///
    /// Deliberately the same condition that runs the capture, so the pill and
    /// the subprocess appear and disappear together — a visible pill always has
    /// live data behind it, and a running capture is always visible. Nothing is
    /// listening to your speakers while there is no pill saying so.
    pub(crate) fn cava_visible(&self) -> bool {
        // A transport needs something to drive. "Anything is making sound" was
        // the right question while the pill was a spectrum — it could visualise
        // a game or an `ffplay` perfectly well — but a row of buttons that
        // presses nothing is worse than an empty band. This also retires the
        // silence-generator problem: `ffplay` publishes no MPRIS, so it no
        // longer keeps the pill on screen forever.
        self.cava_target_player().is_some()
    }

    /// **The one source the whole cluster speaks for.**
    ///
    /// The spectrum shows the *mix* coming out of one sink, which may be two or
    /// three players at once, so the words and the buttons both have to pick
    /// one of them — and critically, the SAME one. They used to choose
    /// independently, which meant the pill could name one player while `[next]`
    /// drove another.
    ///
    /// Scored rather than "first match", because first-match picked wrong on
    /// the dev box: a phone over kdeconnect reports `Playing` with completely
    /// empty metadata, so it beat the Chromium tab that was actually making the
    /// sound, and the pill showed a bare app name.
    ///
    /// * **Playing** outweighs everything — it is the thing you can hear.
    /// * **MPRIS** next. One app shows up twice — once on the bus with its
    ///   track, once as a raw PipeWire stream — because a browser's MPRIS
    ///   belongs to the main process while its audio belongs to a child, so
    ///   the pids never match and the merge cannot fuse them. The MPRIS half is
    ///   the one that knows what is playing; the stream half only knows that
    ///   something is. Without this term the two tied, and `max_by_key` takes
    ///   the LAST match, so the stream won.
    /// * **Owns a window here** next, and this is the one that matters: a phone
    ///   over KDE Connect publishes a perfectly good title for whatever the
    ///   phone is playing, and when it is playing the SAME song it scored
    ///   identically to the browser — so the tie fell to whichever came last,
    ///   and `[play]` drove the phone (2026-09-12). "Has a pid" could not tell
    ///   them apart, because `kdeconnectd` has one; "has a **window**" can,
    ///   which is exactly what the pid→window join was built for.
    /// * **Has a title** breaks whatever is left.
    pub(crate) fn cava_player(&self) -> Option<&options_engine::Playing> {
        let ctx = self.brain.as_ref()?;
        let last = self.cava_last_player.as_deref();
        let score = |p: &options_engine::Playing| -> u8 {
            // ORDER MATTERS, and it is not the obvious one. "Currently making
            // sound" used to outrank everything, which meant pausing handed the
            // whole cluster to `ffplay` — a silence generator that is
            // permanently Playing, owns no window and has no transport at all.
            // The pill named it and `[play]` had nothing to press
            // (Max, 2026-09-12: "i lose the input when i pause").
            //
            // So what the cluster IS comes before what is audible: this is a
            // transport, and a transport belongs to a player you can drive.
            let mpris = u8::from(matches!(
                p.source,
                options_engine::PlayingSource::Mpris { .. }
            )) * 16;
            let windowed = u8::from(ctx.window_of(p).is_some()) * 8;
            let playing = u8::from(p.is_playing()) * 4;
            // **The one you were just listening to.** Pausing used to drop the
            // subject: two paused players tie, the tie falls to list order, and
            // the track you had been playing stops being the one named
            // (Max, 2026-09-12: "i want the last played to stay on info").
            //
            // Ranked BELOW `playing` on purpose — something audible now always
            // outranks something you paused a minute ago — and above `titled`,
            // so among things that are all quiet, memory decides.
            let remembered = u8::from(Self::playing_id(p).as_deref() == last && last.is_some()) * 2;
            let titled = u8::from(!p.title.trim().is_empty());
            mpris + windowed + playing + remembered + titled
        };
        // Strictly-greater, so the FIRST best wins rather than the last.
        // `max_by_key` returns the last maximum, which is how a tie handed the
        // cluster to the phone; the collector's order (MPRIS before streams) is
        // stable, so first-wins is a defined answer rather than a lucky one.
        ctx.playing.iter().fold(
            None,
            |best: Option<&options_engine::Playing>, p| match best {
                Some(b) if score(p) <= score(b) => Some(b),
                _ => Some(p),
            },
        )
    }

    /// A source's stable handle — the MPRIS bus name without its prefix, which
    /// is also what `playerctl -p` takes. `None` for a raw PipeWire stream,
    /// which has no name worth remembering.
    pub(crate) fn playing_id(p: &options_engine::Playing) -> Option<String> {
        match &p.source {
            options_engine::PlayingSource::Mpris { bus } => {
                Some(bus.trim_start_matches("org.mpris.MediaPlayer2.").to_owned())
            }
            _ => None,
        }
    }

    /// The player the cava cluster's transport acts on — the same source the
    /// pill names, as the `playerctl -p` handle.
    ///
    /// `None` when the chosen source is a raw PipeWire stream: an `ffplay` or a
    /// game makes sound but exposes no transport, so there is nothing to press.
    pub(crate) fn cava_target_player(&self) -> Option<String> {
        Self::playing_id(self.cava_player()?)
    }

    /// The `[current data]` child's text: what is playing, in its own words.
    /// Falls back through artist/title to the app's name, because "Firefox" is
    /// a better answer than an empty pill.
    pub(crate) fn cava_now_text(&self) -> String {
        let Some(p) = self.cava_player() else {
            return String::new();
        };
        let (t, a) = (p.title.trim(), p.artist.trim());
        match (t.is_empty(), a.is_empty()) {
            (false, false) => format!("{a} — {t}"),
            (false, true) => t.to_owned(),
            _ => p.app.trim().to_owned(),
        }
    }

    /// The window the current player owns, if it has one — the target for a
    /// keystroke when MPRIS will not do. `None` for a headless player or a
    /// phone, which is also exactly when a keystroke would be meaningless.
    pub(crate) fn cava_player_window(&self) -> Option<String> {
        let ctx = self.brain.as_ref()?;
        let p = self.cava_player()?;
        ctx.window_of(p).map(|w| w.address.clone())
    }

    /// How far through the track the selected player is, 0…1.
    ///
    /// `None` when there is nothing to be a fraction of — a live stream, a radio
    /// station, anything MPRIS reports with no length. A progress bar that
    /// invents a length would be the surface asserting something it does not
    /// know, which is the one thing it may never do.
    pub(crate) fn cava_progress(&self) -> Option<f32> {
        let p = self.cava_player()?;
        (p.length_secs > 0).then(|| (p.position_secs as f32 / p.length_secs as f32).clamp(0.0, 1.0))
    }

    /// `1:23 / 4:56`, or just the position when nothing knows the length.
    pub(crate) fn cava_time_text(&self) -> String {
        let Some(p) = self.cava_player() else {
            return String::new();
        };
        if p.length_secs == 0 && p.position_secs == 0 {
            return String::new();
        }
        let clock = |s: u64| format!("{}:{:02}", s / 60, s % 60);
        if p.length_secs > 0 {
            format!("{} / {}", clock(p.position_secs), clock(p.length_secs))
        } else {
            clock(p.position_secs)
        }
    }

    /// Width the track name may use: what is left after the transport on its
    /// left and the clock on its right. One definition, so the marquee, the
    /// overflow test and the pill's own width cannot disagree.
    pub(crate) fn cava_text_w(&self, rect: Rect) -> f32 {
        let time = if self.cava_time_w > 0.0 {
            self.cava_time_w + CAVA_TIME_GAP
        } else {
            0.0
        };
        (rect.x + rect.w - PILL_PAD_X - time - self.cava_text_x(rect)).max(1.0)
    }

    /// The volume of the default output, as a percentage.
    ///
    /// This is what the output pill shows at rest instead of a speaker glyph
    /// (Max, 2026-09-12): a symbol only says *there is an output*, which you
    /// could guess, while a number says something you cannot. The device's
    /// name is the part worth hiding until asked.
    pub(crate) fn cava_vol_text(&self) -> String {
        match self.cava_vol_pct() {
            Some(pct) => format!("{pct}%"),
            None => String::new(),
        }
    }

    /// The default output's level, as the pill should currently show it.
    ///
    /// A scroll's own result wins until the sink is polled, because the audio
    /// collector runs every 2 s and watching a number you just changed sit
    /// still for two seconds is the opposite of a live readout. Unlike the
    /// play/pause loan this one is *computed*, not guessed — we know the step
    /// we asked for and the range it is clamped to.
    pub(crate) fn cava_vol_pct(&self) -> Option<u32> {
        if let Some((pct, until)) = self.cava_vol_assume {
            if Instant::now() < until {
                return Some(pct);
            }
        }
        let ctx = self.brain.as_ref()?;
        // The sink inventory carries the level for the default device; the
        // older scalar is the fallback for the moment before the first dump.
        Some(
            ctx.default_output()
                .map(|o| o.volume_pct)
                .filter(|v| *v > 0)
                .unwrap_or(ctx.audio.default_sink_volume),
        )
    }

    /// The width of the volume's own slot — a circle unless the number needs
    /// more, which `100%` does. One place, so the layout and the draw cannot
    /// disagree about where the device name starts.
    pub(crate) fn cava_vol_slot(&self) -> f32 {
        (self.cava_vol_w + PILL_PAD_X).max(self.options_pill_h())
    }

    /// The `[output]` child's text: which device the sound is coming out of.
    /// The human description ("sof-hda-dsp Speaker"), never the node name —
    /// `alsa_output.pci-0000_00_1f.3-platform-…` is a fact, not an answer.
    pub(crate) fn cava_out_text(&self) -> String {
        self.brain
            .as_ref()
            .and_then(|c| c.default_output())
            .map(|o| o.description.clone())
            .unwrap_or_default()
    }

    /// The track pill's live rect — which is also the playing box's, since the
    /// box is the pill grown. An accessor rather than opening `Pill`'s fields
    /// to the crate.
    pub(crate) fn cava_now_rect(&self) -> Option<Rect> {
        self.options_pills()
            .iter()
            .find(|p| p.id == PillId::CavaNow)
            .map(|p| p.rect)
    }

    /// Where the now-playing text starts inside its pill: past the transport
    /// cluster it shares the pill with. One definition, used by the layout, the
    /// draw and the marquee, so they cannot disagree about where the words go.
    pub(crate) fn cava_text_x(&self, rect: Rect) -> f32 {
        rect.x + PILL_PAD_X
    }

    /// Where the cava cluster's reveal is heading.
    ///
    /// **`OptionUXRules.md` §2 — "hover growth is reversible only on leave".**
    /// An OPTION that grew because you looked at it stays grown *for the rest
    /// of the visit*, and "the visit ends at the surface, not at the pill:
    /// leaving the clock is not leaving, leaving the bar is."
    ///
    /// Aiming this at the hover alone broke that: moving from the spectrum
    /// toward the clock collapsed the cluster, which un-displaces everything to
    /// its right under a pointer already travelling across it — precisely the
    /// friction §2 was written from. So once open it holds while the pointer is
    /// anywhere on the surface, exactly as the clock's date does.
    pub(crate) fn cava_reveal_target(&self) -> f32 {
        let hovering = self.options_hover.is_some_and(|h| h.is_cava());
        let visiting = self.cava_reveal > 0.01 && self.options_ptr.is_some();
        // An open box holds its own cluster up, whatever the pointer is doing —
        // the list cannot outlive the pill it grew out of.
        f32::from(u8::from(hovering || visiting || self.play_box_open))
    }

    /// Where the output pill's expansion is heading — §2 again, and for the
    /// same reason: it is the widest thing in the cluster, so collapsing it
    /// while the hand is still on the bar drags everything left of it.
    pub(crate) fn cava_out_target(&self) -> f32 {
        let hovering = self.options_hover == Some(PillId::CavaOut);
        let visiting = self.cava_out_t > 0.01 && self.options_ptr.is_some();
        f32::from(u8::from(hovering || visiting))
    }

    /// How present the transport symbols are, 0…1 — and inversely, how far the
    /// bars have faded to make room for them. A crossfade rather than a swap:
    /// the two states are the same pill, so one has to become the other.
    pub(crate) fn cava_sym_alpha(&self) -> f32 {
        // The spectrum is gone (Max, 2026-09-12: "fuck the bars"), and with it
        // the metamorphosis it existed for. The pill IS the transport now, so
        // the symbols are simply always there — no crossfade, no paused face,
        // no reason for the pill to have two states.
        1.0
    }

    /// How much room the track name actually has, and how much of it is missing.
    /// `None` when it fits.
    pub(crate) fn cava_overflow(&self, rect: Rect) -> Option<f32> {
        let over = self.cava_now_w - self.cava_text_w(rect);
        (over > 1.0).then_some(over)
    }

    /// One full turn of the marquee, in px: the title plus the gap before it
    /// comes round again.
    pub(crate) fn cava_scroll_period(&self) -> f32 {
        (self.cava_now_w + CAVA_SCROLL_GAP).max(1.0)
    }

    /// The track text's width as the pill should currently be sized for it —
    /// eased across a change instead of jumping from one song's length to the
    /// next's.
    pub(crate) fn cava_now_w_eased(&self) -> f32 {
        if self.cava_swap <= 0.0 {
            return self.cava_now_w;
        }
        lerp(self.cava_now_w, self.cava_prev_w, self.cava_swap)
    }

    /// The two halves of a title change: `(outgoing_alpha, incoming_alpha)`.
    /// The old words clear out over the first half of the swap and the new ones
    /// arrive over the second, so the pill is never showing two songs at once.
    pub(crate) fn cava_swap_alphas(&self) -> (f32, f32) {
        if self.cava_swap <= 0.0 {
            return (0.0, 1.0);
        }
        // `cava_swap` counts DOWN from 1, so the first half of the change is
        // the top half of the range.
        if self.cava_swap > 0.5 {
            (((self.cava_swap - 0.5) / 0.5).clamp(0.0, 1.0), 0.0)
        } else {
            (0.0, (1.0 - self.cava_swap / 0.5).clamp(0.0, 1.0))
        }
    }

    /// How far the title has slid left, in px.
    ///
    /// A continuous loop: it advances forever and wraps at one period, so there
    /// is no end to arrive at and no journey back. The second copy drawn a
    /// period to the right is what fills the space the first one vacates — the
    /// wrap then happens while the two are interchangeable, and is invisible.
    pub(crate) fn cava_scroll_offset(&self, rect: Rect) -> f32 {
        if self.cava_overflow(rect).is_none() {
            return 0.0;
        }
        (self.cava_scroll * CAVA_SCROLL_SPEED).rem_euclid(self.cava_scroll_period())
    }

    /// Whether the track name is currently too long for its pill — asked of
    /// the LIVE layout rather than of the measured width alone, because the
    /// pill's width is itself animating as the cluster opens.
    pub(crate) fn cava_now_overflows(&self) -> bool {
        self.options_pills()
            .iter()
            .find(|p| p.id == PillId::CavaNow)
            .is_some_and(|p| self.cava_overflow(p.rect).is_some())
    }

    /// Whether the transport shows a pause glyph rather than a play glyph.
    ///
    /// Asked of **the player this cluster speaks for**, not of the machine.
    /// "Is anything playing" was permanently true on the dev box — a silence
    /// generator holds a stream open forever — so the glyph never changed
    /// (Max, 2026-09-12). The button must describe the thing it drives.
    pub(crate) fn cava_is_playing(&self) -> bool {
        // What the click just asked for wins over what the last poll saw, until
        // the bus agrees or the loan expires (see `cava_assume`).
        if let Some((assumed, until)) = self.cava_assume {
            if Instant::now() < until {
                return assumed;
            }
        }
        self.cava_player().is_some_and(|p| p.is_playing())
    }

    /// The cava pill: [`crate::spectrum::BANDS`] bars of whatever is coming out
    /// of the speakers.
    ///
    /// The same material as every other pill — neumorph base, the shared rest
    /// wash — because One Material (§3) is about the substance as well as the
    /// tempo; a visualiser cut from different stuff would read as a widget
    /// somebody dropped on the bar.
    pub(crate) fn push_cava_pill(&self, scene: &mut Scene, rect: Rect) {
        let bright = self.options_bar_is_bright();
        let radius = rect.h / 2.0;
        push_neumorph(scene, rect, radius, bright, 1.0);
        scene.rects.push(RectInst {
            rect,
            radius,
            color: self.options_rest_wash(),
            glass: 0.0,
            border: 0.0,
        });
    }

    /// One of the cava pill's revealed children — the track name, the output,
    /// or one of the three transport symbols drawn on the pill itself.
    ///
    /// The sliding children fade in a touch *after* the slide starts, the same
    /// way the clipboard's copy-link does, so they read as coming out from
    /// under the transport pill rather than blinking on beside it.
    fn push_cava_child(&self, scene: &mut Scene, pill: &Pill) {
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(pill.id);
        let rect = pill.rect;
        let ink = self.options_text_color();
        // The three transport symbols live INSIDE the now-playing pill and draw
        // no ground of their own — no neumorph, no wash capsule. They are marks
        // on the pill, not pills on the bar (Max, 2026-09-12). Hover is carried
        // by the ink alone: a symbol you can act on brightens, and the pill
        // beneath it stays still.
        let is_symbol = matches!(
            pill.id,
            PillId::CavaPlay | PillId::CavaPrev | PillId::CavaNext
        );
        // The symbols ride the METAMORPHOSIS, the sliding children ride the
        // REVEAL. Two different clocks, and they must be chosen before anything
        // returns: an early exit on the reveal used to kill the symbols of a
        // PAUSED pill, which is not revealed at all — so a paused player showed
        // neither bars nor buttons (Max, 2026-09-12).
        let a = if is_symbol {
            self.cava_sym_alpha()
        } else {
            ((self.cava_reveal - 0.15) / 0.6).clamp(0.0, 1.0)
        };
        if a <= 0.01 {
            return;
        }
        if !is_symbol {
            // The track pill becomes the playing box. Same morph the clipboard
            // makes: the radius runs from the stadium to the box's corner, and
            // opacity LEADS the height — without that lead the fill reads as a
            // translucent ghost inflating instead of a solid panel swelling.
            let e = if pill.id == PillId::CavaNow {
                self.play_box_e
            } else {
                0.0
            };
            let solid = 1.0 - (1.0 - e).powi(3);
            // From the BAND's radius, not this rect's. `radius` above is
            // `rect.h / 2`, and `rect.h` is the GROWING height — so the corner
            // swelled with the box and the thing ballooned into a lozenge
            // before it opened. The clipboard lerps from the resting band's
            // `ph / 2`, which is a constant, and that is the whole difference.
            let radius = lerp(self.options_pill_h() / 2.0, crate::clipboard::BOX_RADIUS, e);
            push_neumorph(scene, rect, radius, bright, a);
            let wash = if hovered && e < 0.01 {
                self.options_hover_wash()
            } else {
                self.options_rest_wash()
            };
            // The box fill is the pill's wash grown into the panel colour, so a
            // half-open box is never a colour the bar does not otherwise hold.
            let (fill, _) = self.clip_box_surface();
            let color = lerp4(
                [wash[0], wash[1], wash[2], wash[3] * a],
                [fill[0], fill[1], fill[2], self.box_panel_alpha() * a],
                solid,
            );
            scene.rects.push(RectInst {
                rect,
                radius,
                color,
                glass: 0.0,
                border: 0.0,
            });
            if pill.id == PillId::CavaNow {
                // The pill fills as the track plays. Drawn over the wash and
                // under the words — rects all precede labels — so it reads as
                // the pill's own substance rising rather than a bar laid on it.
                //
                // Only while it is still a PILL: once it has grown into the
                // list, a fill climbing across a panel of rows would be
                // meaningless, and the band it belongs to is no longer the
                // whole shape.
                if let Some(prog) = self.cava_progress().filter(|_| e < 0.5) {
                    let r = self.options_pill_h() / 2.0;
                    // Never narrower than its own cap, so a track at 0:02 is a
                    // dot at the left rather than a sliver with no shape.
                    let fw = (rect.w * prog).max(2.0 * r).min(rect.w);
                    let ink = self.options_text_color();
                    scene.rects.push(RectInst {
                        rect: Rect::new(rect.x, rect.y, fw, self.options_pill_h()),
                        radius: r,
                        color: [ink[0], ink[1], ink[2], 0.14 * a],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                self.push_play_rows(scene, rect, solid * a);
            }
        }

        let s = self.options_scale();
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);

        // The volume: always present, always in the same place, whether the
        // shape around it is a circle or a pill. It is the thing that does NOT
        // move while the device name grows out beside it — the clock's date
        // works the same way, and it is what makes the change read as one
        // object opening rather than two objects swapping.
        if pill.id == PillId::CavaOut {
            let slot = self.cava_vol_slot();
            scene.labels.push(Label {
                text: self.cava_vol_text(),
                pos: (rect.x + slot / 2.0, rect.y + (rect.h - line_px) / 2.0),
                max_w: slot,
                font_px,
                line_px,
                centered: true,
                dim: false,
                cache: false,
                clip: None,
                family: TEXT_FONT,
                color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            });
        }

        // A symbol is centred in its own hit-slot; the output's name starts
        // past its glyph; the now-playing text starts past the transport
        // cluster it shares a pill with.
        let centered = is_symbol;
        let pos_x = if is_symbol {
            rect.x + rect.w / 2.0
        } else if pill.id == PillId::CavaNow {
            // Slid left by the marquee when the title does not fit.
            self.cava_text_x(rect) - self.cava_scroll_offset(rect)
        } else if pill.id == PillId::CavaOut {
            rect.x + self.cava_vol_slot()
        } else {
            rect.x + PILL_PAD_X
        };
        // The name arrives on the BACK half of the opening, so it appears in a
        // pill that is already a pill rather than being squeezed through a
        // circle on the way.
        let a = if pill.id == PillId::CavaOut {
            a * ((self.cava_out_t - 0.45) / 0.45).clamp(0.0, 1.0)
        } else {
            a
        };
        if a <= 0.01 {
            return;
        }
        // A resting symbol sits back; the one under the pointer comes forward.
        let ink = if is_symbol && !hovered {
            [ink[0], ink[1], ink[2], ink[3] * 0.72]
        } else {
            ink
        };
        // The text may only use what is left of the pill AFTER whatever sits to
        // its left — for the now-playing pill that is the transport cluster.
        // Measured from where the text actually starts rather than from the
        // pill's edge, so the two can never disagree.
        // The trailing time, right-aligned in the band — the same place the
        // clipboard row puts its relative time.
        if pill.id == PillId::CavaNow && self.cava_time_w > 0.0 {
            let band = self.options_pill_h();
            scene.labels.push(Label {
                text: self.cava_time_text(),
                pos: (
                    rect.x + rect.w - PILL_PAD_X - self.cava_time_w,
                    rect.y + (band - line_px) / 2.0,
                ),
                max_w: self.cava_time_w.max(1.0),
                font_px,
                line_px,
                centered: false,
                dim: true,
                cache: false,
                clip: Some(Rect::new(rect.x, rect.y, rect.w, band)),
                family: pill.family,
                color: Some(self.dim_ink(ink)),
            });
        }
        let max_w = (rect.x + rect.w - PILL_PAD_X - pos_x).max(0.0);
        // Clipped to its own pill. A width can go stale for one frame — a track
        // changes between a measure and a draw — and when it does the text must
        // be cut off, never painted across the bar.
        //
        // The now-playing text is clipped tighter, to the TEXT column alone,
        // because it scrolls: bounded by the whole pill it would slide out from
        // under the transport symbols, and text creeping out from behind the
        // buttons reads as a glitch rather than as a marquee.
        let clip = if pill.id == PillId::CavaNow {
            // The text column alone, and only the BAND's height: the marquee
            // must not slide under the transport on its left, past the time on
            // its right, or down into the list below it.
            Some(Rect::new(
                self.cava_text_x(rect),
                rect.y,
                self.cava_text_w(rect),
                self.options_pill_h(),
            ))
        } else {
            Some(rect)
        };
        // A title being retired: the previous words fading out in place while
        // the new ones wait their turn. Drawn before everything else so the
        // arriving title lands over them.
        let (out_a, in_a) = self.cava_swap_alphas();
        if pill.id == PillId::CavaNow && out_a > 0.01 && !self.cava_prev_text.is_empty() {
            scene.labels.push(Label {
                text: self.cava_prev_text.clone(),
                pos: (self.cava_text_x(rect), rect.y + (rect.h - line_px) / 2.0),
                max_w: self.cava_prev_w.max(1.0),
                font_px,
                line_px,
                centered: false,
                dim: false,
                cache: false,
                clip,
                family: pill.family,
                color: Some([ink[0], ink[1], ink[2], ink[3] * a * out_a]),
            });
        }
        // While a swap is running the arriving title carries its own fade; the
        // rest of the time it is simply present.
        let a = if pill.id == PillId::CavaNow {
            a * in_a
        } else {
            a
        };
        if a <= 0.01 {
            return;
        }
        // The loop's second copy, one period to the right, drawn first so the
        // leading copy overlaps it rather than the other way round. Without it
        // the pill would go blank for the moment the title has left but has not
        // yet re-entered — and a marquee that blinks is a marquee that stutters.
        if pill.id == PillId::CavaNow && self.cava_overflow(rect).is_some() {
            let ty = rect.y + (rect.h - line_px) / 2.0;
            scene.labels.push(Label {
                text: pill.text.clone(),
                pos: (pos_x + self.cava_scroll_period(), ty),
                max_w: self.cava_now_w.max(1.0),
                font_px,
                line_px,
                centered: false,
                dim: false,
                cache: false,
                clip,
                family: pill.family,
                color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            });
        }
        scene.labels.push(Label {
            text: pill.text.clone(),
            pos: (pos_x, rect.y + (rect.h - line_px) / 2.0),
            // A scrolling title must be allowed to lay out at its FULL width —
            // bounding it to the visible column would wrap or truncate it, and
            // then there would be nothing left to scroll to.
            max_w: if centered {
                rect.w
            } else if pill.id == PillId::CavaNow {
                self.cava_now_w.max(max_w)
            } else {
                max_w
            },
            font_px,
            line_px,
            centered,
            dim: false,
            cache: false,
            clip,
            family: pill.family,
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
        });
    }

    pub(crate) fn push_options_tooltip(&self, scene: &mut Scene) {
        // The stage's mode switch wears a glyph and no word, so hovering is
        // where it says what it is — and, since the glyph is the mode you are
        // in, what a click would do.
        if self.options_hover == Some(PillId::StageMode) {
            let label = match self.stage.mode() {
                crate::stage::Mode::Task => {
                    crate::i18n::tr("Showing one task — show the whole desk")
                }
                crate::stage::Mode::Desk => {
                    crate::i18n::tr("Showing the whole desk — show one task")
                }
            };
            self.push_tooltip_at(scene, PillId::StageMode, label.to_owned());
            return;
        }
        let Some(PillId::Option(i)) = self.options_hover else {
            return;
        };
        let Some(label) = self
            .surfaced_options()
            .get(i as usize)
            // Engine titles are user-visible text: translate at display
            // (English literal = key), like every daemon-side string. The
            // detail rides along when present — it's the only place an
            // offer's specifics ("Reclaim 1.2 GB", the branch, the copied
            // word) ever reach the screen. Details are data (paths, sizes),
            // so they pass through untranslated, capped so a full URL can't
            // balloon the tooltip.
            .map(|a| {
                let title = crate::i18n::tr_dyn(&a.title).to_owned();
                let detail = a.detail.trim();
                if detail.is_empty() || detail == title {
                    title
                } else {
                    format!("{title} — {}", truncate(detail, 40))
                }
            })
            .filter(|s| !s.trim().is_empty())
        else {
            return;
        };
        self.push_tooltip_at(scene, PillId::Option(i), label);
    }

    /// Draw one tooltip under the named pill. Split out so any pill can have
    /// one — the box is the same object wherever it hangs, and only the pill it
    /// points at and the words in it change.
    fn push_tooltip_at(&self, scene: &mut Scene, id: PillId, label: String) {
        let pills = self.options_pills();
        let Some(pr) = pills.iter().find(|p| p.id == id).map(|p| p.rect) else {
            return;
        };
        let bar_h = self.options_bar_h();
        let full_w = self.options_size.0 as f32;
        // Rides the pill scale so the tooltip matches the pills it names.
        let s = self.options_scale();
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);
        // The Label shapes exactly at render time; only the background needs a
        // size, so an estimate is fine here — but it must be an OVERestimate:
        // the label shapes into a one-line box of max_w, and anything wider
        // WRAPS to an invisible second line (found live: an all-caps
        // translated title rendered as its first word). Char-class aware, so
        // caps-heavy strings don't blow past a flat per-char average.
        let pad_x = 11.0;
        let text_w = est_text_w(&label, font_px);
        let box_w = (text_w + 2.0 * pad_x).min((full_w - 8.0).max(24.0));
        let box_h = line_px + 9.0;
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
            pos: (bx + box_w / 2.0, by + (box_h - line_px) / 2.0),
            // Full box width (pads included) as the wrap limit: if the
            // estimate is ever slightly tight, the line eats into the pill's
            // rounded ends instead of wrapping out of existence.
            max_w: box_w,
            font_px,
            line_px,
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
        let bar_h = self.options_bar_h();
        let full_w = self.options_size.0 as f32;
        // Pill text at the bar's live scale — the same size `measure_options_text`
        // measured against, so text always fits the pill that was sized for it.
        let s = self.options_scale();
        let (font_px, line_px) = (FONT_PX * s, LINE_PX * s);

        let pills = self.options_pills();
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
            if pill.id == PillId::Cava {
                self.push_cava_pill(scene, pill.rect);
                continue;
            }
            if pill.id.is_cava() {
                self.push_cava_child(scene, pill);
                continue;
            }
            // The sunset prompt's nested [turn on]: the SAME material as its
            // parent — the module's own flat fill (glass:0.0, no rim/fresnel/
            // iridescence), not the dock's liquid glass — so the button is
            // cut from the same substance as the module, set apart only by a
            // subtle hairline border and, on hover, the standard wash. Its
            // presence rides the module's title morph rather than being a
            // layout fact.
            // Colour AND material match the parent pill (Max, 2026-09-08:
            // "the gear and turn on buttons... are the dock material" — same
            // box fill at the same alpha was still reading as a different
            // substance while `glass: 1.0` ran the dock's shader on top of
            // it; flat fill is what the parent itself uses now).
            if matches!(pill.id, PillId::SunsetTurnOn | PillId::ModuleSettings) {
                let mut a = self.module_child_alpha() * self.options_pill_fade(pill.id, pill.rect);
                // [turn on] fades out as the settings box opens; the gear stays
                // (it is the box's own close/settings affordance, top-right).
                if pill.id == PillId::SunsetTurnOn {
                    a *= 1.0 - self.module_box_e;
                }
                if a > 0.01 {
                    // Ink measured against the module it stands on, not the
                    // bar — same adaptive rule, right surface.
                    let ink = self.module_ink();
                    // It holds its size under the pointer — hover speaks with
                    // the wash alone, not a lift, so the button sits steady
                    // inside the module.
                    let hovered = self.options_hover == Some(pill.id);
                    let rect = pill.rect;
                    let radius = rect.h / 2.0;
                    // The parent's material, again: glass fill, in the box's
                    // own fill colour AND alpha (`module_fill_alpha`) — the
                    // exact pair the parent pill uses — rather than the
                    // theme's raw background at its own separate opacity
                    // (Max, 2026-09-08: "the child[ren], the same color as
                    // the parent" — `bfill`'s alpha is always 1.0, which read
                    // more solid than the parent once the parent's own alpha
                    // stopped being 1.0 too).
                    let (bfill, _) = self.options_box_surface();
                    let fa = self.module_fill_alpha();
                    scene.rects.push(RectInst {
                        rect,
                        radius,
                        color: [bfill[0], bfill[1], bfill[2], fa * a],
                        glass: 0.0,
                        border: 0.0,
                    });
                    // Hover feedback on top, as every pill gets.
                    if hovered {
                        scene.rects.push(RectInst {
                            rect,
                            radius,
                            color: [
                                hover_wash[0],
                                hover_wash[1],
                                hover_wash[2],
                                hover_wash[3] * a,
                            ],
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    // A subtle hairline in the module's own ink, so the button
                    // reads as an inset of the same glass rather than floating.
                    scene.rects.push(RectInst {
                        rect,
                        radius,
                        color: [ink[0], ink[1], ink[2], SUNSET_BORDER_A * a],
                        glass: 0.0,
                        border: (1.2 * s).max(1.0),
                    });
                    scene.labels.push(Label {
                        text: pill.text.clone(),
                        pos: (rect.x + rect.w / 2.0, rect.y + (rect.h - line_px) / 2.0),
                        max_w: rect.w,
                        font_px,
                        line_px,
                        centered: true,
                        dim: false,
                        cache: true,
                        family: pill.family,
                        color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
                        clip: None,
                    });
                }
                continue;
            }
            // Reveal animation for the control buttons: slide out horizontally
            // from behind the parent's near edge (slide 0 = tucked, 1 = rest),
            // fading in; the glyph is clipped to the emerge side so it reads as
            // coming out from under the parent rather than through it.
            // A sticky control is not riding the reveal chain — it is standing
            // on its own on a concealed bar (`OptionUXRules.md` §4), so it
            // draws at full presence instead of tucking back behind a parent
            // that is no longer there.
            let (rect, a, clip, shadow_a) = match ctrl_index(pill.id) {
                Some(i) if self.options_sticky.is_none() => {
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
                    // Read from the LAYOUT rather than from a fixed chain of
                    // ids: the row's membership changes now (the mode the
                    // window is in is shown as a word instead of a toggle), so
                    // "behind the previous pill" has to mean whichever pill is
                    // actually to the left — the close, the state word, or
                    // another toggle.
                    let prev_right = pills
                        .iter()
                        .filter(|p| {
                            matches!(
                                p.id,
                                PillId::Close
                                    | PillId::WindowState
                                    | PillId::Pseudo
                                    | PillId::Float
                                    | PillId::Fullscreen
                            ) && p.rect.x < pill.rect.x
                        })
                        .map(|p| p.rect.x + p.rect.w)
                        .fold(f32::MIN, f32::max);
                    let edge = if prev_right > f32::MIN {
                        prev_right
                    } else {
                        pill.rect.x
                    };
                    let (origin, edge) = (edge - d, edge);
                    let x = lerp(origin, pill.rect.x, t);
                    let rect = Rect::new(x, pill.rect.y, d, pill.rect.h);
                    let clip = Rect::new(edge, 0.0, (full_w - edge).max(0.0), bar_h);
                    // Gate the depth shadow by slide so a tucked button's halo
                    // doesn't leak over the parent (overlay shadows draw on top).
                    (rect, a, Some(clip), a * t)
                }
                _ => (pill.rect, 1.0, None, 1.0),
            };
            // Unified hover lift: a hovered pill grows a touch (drawn geometry
            // only) so hover reads as a tactile rise, not a shrink.
            // The bar's own fade, applied per pill so the sticky pair can be
            // exempt from it (`OptionUXRules.md` §3 for the tempo, §4 for who
            // is allowed to stay).
            let fade = self.options_pill_fade(pill.id, pill.rect);
            if fade <= 0.001 {
                continue;
            }
            let a = a * fade;
            let shadow_a = shadow_a * fade;
            let hovered = self.options_hover == Some(pill.id);
            // The hover lift reads as a RISE only when a bar full of pills sets
            // the baseline. A sticky pair has no baseline — two circles alone
            // on a concealed bar — so the lifted one just makes its partner
            // look shrunken, and the doorway stops reading as a full OPTION
            // (`OptionUXRules.md` §4: it is an OPTION, identical to the others,
            // only empty). There, hover speaks with colour alone.
            // The asking sunset module holds still under the pointer — its
            // size step is a statement, not the hover lift, and lifting it
            // would blur the two; only its nested [turn on] answers hover.
            let module_t = if pill.id == PillId::Window {
                self.module_t()
            } else {
                0.0
            };
            // The gear's readout holds its size under the pointer for the same
            // reason the asking module does: it is a panel that slid out, and a
            // panel that also grows on hover reads as two animations arguing.
            let hovered = hovered && pill.id != PillId::SettingsStats;
            let rect = if hovered && module_t <= 0.001 && self.options_sticky.is_none() {
                hover_grow(rect)
            } else {
                rect
            };
            // Stadium (h/2) normally; but the sunset module, as it opens into
            // the settings box, morphs its corners from the pill's stadium to a
            // small ROUNDED-RECTANGLE radius (like the dock's open card, only
            // smaller) so the box is a panel, not a giant lozenge. The stadium
            // is measured against the pill BAND height, not the grown box
            // height, so it doesn't balloon as the box drops.
            let radius = if pill.id == PillId::Window && module_t > 0.001 {
                let band_h = self.options_pill_h() + MODULE_GROW_H * module_t;
                lerp(
                    band_h / 2.0,
                    MODULE_PANEL_RADIUS * self.options_scale(),
                    self.module_box_e,
                )
            } else if pill.id == PillId::SettingsStats {
                // The readout's row is a stadium; its panel is a rounded
                // RECTANGLE, or a tall box measured against its own height comes
                // out a giant lozenge (the trap the sunset module hit first).
                // The stadium is measured against the BAND, never the grown box.
                self.stats_radius()
            } else {
                rect.h / 2.0 // stadium ⇒ circle when w == h
            };
            // The module swaps its pill costume for waverunner's own material
            // (drawn in the message branch below): the ordinary neumorph and
            // wash fade out as that material fades in — one substance at a
            // time, never stacked.
            push_neumorph(scene, rect, radius, bright, shadow_a * (1.0 - module_t));
            let base = if hovered { hover_wash } else { rest_wash };
            // A pill's wash is a translucent film — right for a pill, wrong for
            // a panel, which has to occlude what it grew over. The readout lerps
            // to the same frosted surface the other boxes use as it opens.
            let (base, alpha) = if pill.id == PillId::SettingsStats && self.stats_open_t() > 0.001 {
                let e = self.stats_open_t();
                // Its own side's frost — the gear lives at the left edge beside
                // the clipboard, not at the notification's end of the bar.
                let (fill, _) = self.box_surface_at(rect);
                (fill, lerp(base[3] * a, self.box_panel_alpha(), e))
            } else {
                (base, base[3] * a * (1.0 - module_t))
            };
            scene.rects.push(RectInst {
                rect,
                radius,
                color: [base[0], base[1], base[2], alpha],
                glass: 0.0,
                border: 0.0,
            });
            let g = pill.glyph_color.unwrap_or(text_color);
            let family = pill.family;
            let cx = rect.x + rect.w / 2.0;
            let ty = rect.y + (rect.h - line_px) / 2.0;
            let mk = |text: String, alpha: f32, max_w: f32, clip: Option<Rect>| Label {
                text,
                pos: (cx, ty),
                max_w,
                font_px,
                line_px,
                centered: true,
                dim: false,
                cache: true,
                family,
                // `a` is the pill's own presence (the bar's reveal, its slide,
                // whatever is fading it); `alpha` is this label's part in a
                // crossfade. A glyph drawn at full opacity on a pill that is
                // fading out is the pill's body vanishing and its icon staying —
                // seen live when the gear's readout slid over the clipboard
                // (2026-09-13). Every text on this bar rides its pill.
                color: Some([g[0], g[1], g[2], g[3] * alpha * a]),
                clip,
            };
            // The clock pill crossfades HH:MM ↔ the full date during its
            // metamorphosis: the clock fades out early, the date fades in late
            // (a slight overlap), the date centred on the pill and scissor-
            // clipped to it so it reveals from the centre outward as it grows.
            // The window pill crossfades the outgoing title into the incoming
            // one while its width eases between them — the same split as the
            // clock. Each label keeps its own natural width so neither wraps
            // mid-morph; the scissor clip does the reveal. Under the Leader the
            // pill's leader-facing edge is pinned, so this plays out entirely
            // on its far side (`OptionUXRules.md` §1).
            let tt = self.options_title_meta.t;
            // The window pill while a MODULE is on it (arriving, standing, or
            // leaving). A module with nested controls lays its sentence out
            // LEFT-anchored, because [turn on] owns the module's right end; a
            // childless one centres it like an ordinary title, since nothing else
            // is competing for the width. Each label keeps its own anchor through
            // the crossfade; the scissor clip does the reveal as always.
            let module = self.module_drawn();
            // The sentence being drawn is the one in the metamorphosis, not a
            // constant: a module's line can change while it stands (the empty
            // room reports whatever is most worth saying).
            let meta = &self.options_title_meta;
            let msg = module.map(|_| {
                if meta.shown_module.is_some() {
                    meta.shown.as_str()
                } else {
                    meta.outgoing.as_str()
                }
            });
            // Arriving or settled when the module's sentence is what the pill
            // SHOWS; leaving when it is what the pill is coming FROM (which is
            // also how one module handing the pill to another reads — the
            // incoming module's own branch, with the outgoing sentence fading out
            // as an ordinary title would).
            let msg_in = msg == Some(self.options_title_meta.shown.as_str());
            if pill.id == PillId::Window && module.is_some() {
                // waverunner's own coat — the dock card's material verbatim:
                // its soft drop shadow, then the theme background over the
                // liquid-glass pipeline (rim glow, fresnel, iridescence, the
                // bottom vignette — every effect rides `glass: 1.0`). The
                // compositor's layer blur reads through the tint's alpha
                // exactly as it does under the dock. Presence animates through
                // the tint, so the material is whole at every morph step.
                let mt = self.module_t();
                // LAYER 1 — the BANNER BEHIND, shape-shifting to sit under the
                // pill. The bar's OWN material (`options_bar_fill`) is
                // smooth-unioned with the bar edge (shader neck, flagged by
                // MODULE_NECK_GLASS), so it bulges DOWN out of the bar to a
                // blister a small rim larger than the pill — the pill always
                // sits ON this banner, and the banner changes shape with it
                // (grows as the box opens). Quad = pill + rim + neck margin; the
                // shader insets the SDF back by the neck `k` to (pill + rim).
                let (banner, _) = self.options_bar_fill();
                let rim = MODULE_BANNER_RIM * s;
                let nk = MODULE_NECK_K * s;
                let brect = Rect::new(
                    rect.x - rim - nk,
                    rect.y - rim - nk,
                    rect.w + 2.0 * (rim + nk),
                    rect.h + 2.0 * (rim + nk),
                );
                scene.rects.push(RectInst {
                    rect: brect,
                    radius: radius + rim,
                    color: [banner[0], banner[1], banner[2], banner[3] * mt],
                    glass: MODULE_NECK_GLASS,
                    border: 0.0,
                });
                // LAYER 2 — the pill/box ON TOP, in the REGULAR OPTIONS pill
                // material (neumorph + the bar wash), just bigger — not the
                // dock glass. Like the notif/clipboard OPTIONS it morphs from
                // the small-pill rest-wash (closed) to the frosted box panel
                // (`options_box_surface`) as it opens, so a big open box still
                // occludes/frosts instead of being a see-through wash. Sits on
                // the banner blister behind it.
                let bright = self.options_bar_is_bright();
                // The module can stand anywhere along the bar, so it reads the
                // frost of the side it is actually on (`box_surface_at`).
                let (bfill, _) = self.box_surface_at(rect);
                let fa = self.module_fill_alpha();
                push_neumorph(scene, rect, radius, bright, mt);
                scene.rects.push(RectInst {
                    rect,
                    radius,
                    // Colour: the SAME as the open box's own fill at every
                    // step, not just once it's fully open (Max, 2026-09-08:
                    // "make it the same color than action box is now") — the
                    // alpha boost above already makes the resting pill read
                    // as opaque as the box, so its colour should match too
                    // rather than leaving a wash tint behind at rest.
                    color: [bfill[0], bfill[1], bfill[2], fa * mt],
                    glass: 0.0,
                    border: 0.0,
                });
                let ink = self.module_ink();
                let msg_text = msg.unwrap_or_default();
                let msg_w = self.module_text_w + 2.0;
                let out = (1.0 - tt / TITLE_OUT_END).clamp(0.0, 1.0);
                let inn = ((tt - TITLE_IN_START) / (1.0 - TITLE_IN_START)).clamp(0.0, 1.0);
                // As the settings box opens the message fades out and stays
                // pinned to the top band (it must not slide to the centre of the
                // growing panel); the settings content fades in below it.
                let bf = 1.0 - self.module_box_e;
                let band_ty = rect.y + (self.options_pill_h() + MODULE_GROW_H * mt - line_px) / 2.0;
                // Where the sentence sits, and how far it may run.
                //
                // With children, LEFT-anchored, and its clip stops at `[turn
                // on]`'s left edge rather than the module's outer edge: during
                // the arrival morph the box is still narrower than its settled
                // width and `[turn on]` sits well inside it, so clipping to
                // `rect` alone lets the sentence (drawn at its fixed final width
                // throughout) run straight through the button until the box
                // nearly catches up (found live 2026-09-08 — see
                // `module_nested_rects`).
                //
                // Childless, CENTRED in the pill and clipped to it, exactly like
                // an ordinary title: there is no right end to keep clear, and a
                // sentence pushed left in a pill that is only as wide as the
                // words would read as a layout mistake.
                let (msg_cx, msg_clip) = if module.is_some_and(Module::has_children) {
                    let clip_w = (self.module_msg_right() - rect.x).clamp(0.0, rect.w);
                    (
                        rect.x + PILL_PAD_X + self.module_text_w / 2.0,
                        Rect::new(rect.x, rect.y, clip_w, rect.h),
                    )
                } else {
                    (cx, rect)
                };
                // The sentence in the slab's own ink, so text and surface are
                // measured for each other exactly as inside the boxes.
                let mk_at = |text: String, alpha: f32, at_cx: f32, max_w: f32| Label {
                    pos: (at_cx, band_ty),
                    color: Some([ink[0], ink[1], ink[2], ink[3] * alpha * bf]),
                    ..mk(text, alpha, max_w, Some(msg_clip))
                };
                if tt >= 0.999 {
                    // Settled on the module's sentence.
                    scene
                        .labels
                        .push(mk_at(msg_text.to_owned(), 1.0, msg_cx, msg_w));
                } else if msg_in {
                    // The old title (or the module handing over) fades out from
                    // its centre; the sentence fades in as the module widens.
                    if out > 0.001 {
                        scene.labels.push(mk(
                            self.options_title_meta.outgoing.clone(),
                            out,
                            self.options_title_meta.from + 2.0,
                            Some(rect),
                        ));
                    }
                    if inn > 0.001 {
                        scene
                            .labels
                            .push(mk_at(msg_text.to_owned(), inn, msg_cx, msg_w));
                    }
                } else {
                    // The sentence fades out; the returning task title fades in
                    // centred.
                    if out > 0.001 {
                        scene
                            .labels
                            .push(mk_at(msg_text.to_owned(), out, msg_cx, msg_w));
                    }
                    if inn > 0.001 {
                        scene.labels.push(mk(
                            pill.text.clone(),
                            inn,
                            self.options_title_w + 2.0,
                            Some(rect),
                        ));
                    }
                }
                continue;
            }
            if pill.id == PillId::Window && tt < 0.999 {
                let out = (1.0 - tt / TITLE_OUT_END).clamp(0.0, 1.0);
                let inn = ((tt - TITLE_IN_START) / (1.0 - TITLE_IN_START)).clamp(0.0, 1.0);
                if out > 0.001 {
                    scene.labels.push(mk(
                        self.options_title_meta.outgoing.clone(),
                        out,
                        self.options_title_meta.from + 2.0,
                        Some(rect),
                    ));
                }
                if inn > 0.001 {
                    scene.labels.push(mk(
                        pill.text.clone(),
                        inn,
                        self.options_title_w + 2.0,
                        Some(rect),
                    ));
                }
                continue;
            }
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
        // `floating` rides along for the state pill: the engine already watches
        // it, so the bar can notice a window leaving the layout without asking
        // the compositor anything. Where it cannot be known (no engine), it is
        // reported as whatever the bar already believes, so the comparison
        // below stays quiet rather than triggering a read every tick.
        let (addr, title, class, fullscreen, floating) = match self.brain.as_ref() {
            Some(ctx) if crate::brain::hypr_alive(ctx) => {
                let w = &ctx.window;
                if w.address.is_empty() {
                    (None, None, None, false, false)
                } else {
                    (
                        Some(w.address.clone()),
                        Some(w.title.clone()),
                        Some(w.class.clone()),
                        w.is_fullscreen,
                        w.is_floating,
                    )
                }
            }
            _ => {
                let believed = self.options_mode == hypr::WindowMode::Floating;
                match hypr::active_window_info() {
                    Some((a, t, fs)) => {
                        let class = hypr::active_window_where().map(|(c, _)| c);
                        (Some(a), Some(t), class, fs, believed)
                    }
                    None => (None, None, None, false, believed),
                }
            }
        };
        // The class travels with the title: the grooming that turns a title into
        // a task needs the app's own name to recognise its signature
        // ([`crate::task_title`]), so it must be as current as the title it
        // grooms — not only as current as the last focus change. It counts as a
        // change to the pill in its own right: an app that renames its class
        // under a standing title changes what the pill is allowed to strip.
        let class_changed = self.options_class != class;
        self.options_class = class.clone();
        if self.options_active_addr != addr {
            // Focus moved, so a sticky OPTION's way back is stale: it offers to
            // undo something you are no longer looking at, which is worse than
            // offering nothing (`OptionUXRules.md` §4).
            self.clear_sticky();
            // Focus moved — re-derive the copy-link affordance from the new app's
            // class (only browsers expose a copyable page URL).
            let is_browser = class.is_some_and(|c| hypr::is_browser_class(&c));
            self.set_clip_link_available(is_browser);
            // Feed the usage-aware focus cycle (walk-driven hops excluded).
            if let Some(a) = addr.clone() {
                self.note_focus_change(&a);
            }
            // And remember where the user is on this workspace, so arriving
            // back by swipe hands the space over intact.
            self.note_ws_focus();
        }
        // What the window IS, for the state pill. Re-read only when the answer
        // can have changed without us doing it: focus moved, or the engine
        // disagrees with the cached mode about being fullscreen or floating —
        // which is how a window that leaves the layout by itself announces
        // itself (a video going full-screen, a dialog floating, a rule). Pseudo
        // needs no watching: it is our own tag, and the only way in or out is
        // `set_window_mode`, which refreshes as it goes. So an ordinary context
        // tick costs no compositor read at all.
        let stale = match self.options_mode {
            hypr::WindowMode::Fullscreen => !fullscreen,
            hypr::WindowMode::Floating => !floating,
            _ => fullscreen || floating,
        };
        if self.options_active_addr != addr || stale {
            // Which also notices a window joining or leaving the layout, and
            // asks the solitary-pseudo rule to look — see `refresh_window_mode`.
            self.refresh_window_mode();
        }
        if self.options_active_addr != addr || self.options_title != title || class_changed {
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
            self.set_options_hidden(fs && !self.overview_active);
            if fs {
                // The bar is being taken away. If the user's own click is what
                // took it, leave that control standing (`OptionUXRules.md` §4)
                // — otherwise the hover is dropped as before.
                if !self.arm_sticky() {
                    self.options_hover = None;
                }
            } else {
                // Back from fullscreen: the bar itself is the way back now.
                self.clear_sticky();
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
            self.bar_want = None;
            self.dock_bar_matched = None;
            self.dock_want = None;
            self.clip_want = None;
        } else {
            // Back from fullscreen: resume the colour-match cadence.
            self.schedule_options_poll();
            self.reeval_options_bar();
            self.reeval_dock_bar();
        }
    }

    /// Whether the topbar's screencopy colour-match should be paused: a
    /// fullscreen client is focused and the overview (which shows the bar on its
    /// own strip) isn't up.
    pub(crate) fn options_paused(&self) -> bool {
        self.options_fullscreen && !self.overview_active
    }

    /// Leave the acted-on control standing on the concealed bar, with its
    /// doorway — Sticky OPTIONS (`OptionUXRules.md` §4). Returns whether
    /// anything stuck.
    ///
    /// The trigger is a condition, not a list: the bar is concealing, the user
    /// acted on it a moment ago, and their pointer is still there. Any future
    /// OPTION whose action hides the bar inherits this without knowing about it.
    fn arm_sticky(&mut self) -> bool {
        let Some((id, rect, when)) = self.options_acted else {
            return false;
        };
        // Concealed for its own reasons, or the hand has already gone: there is
        // no journey to save.
        if when.elapsed() > STICKY_BLAME || !self.options_ptr_on_bar() {
            return false;
        }
        // The doorway stands in its successor's slot, on the side the rest of
        // the group lives — for [fullscreen] that is leftward, toward
        // [pseudo], [X] and the title.
        let step = rect.w + CTRL_GAP;
        let leftward = self.group_lies_left_of(id, rect);
        let door_x = if leftward {
            rect.x - step
        } else {
            rect.x + step
        };
        self.options_sticky = Some(Sticky {
            id,
            rect,
            door: Rect::new(door_x, rect.y, rect.w, rect.h),
            ctrl: self.options_ctrl.t,
            ctrl_reveal: self.options_ctrl.reveal,
        });
        // The pointer is on it — that was a precondition of getting here — so
        // it is hovered, without waiting for the next motion to say so. The
        // hand never left the control; the bar left from under the hand.
        self.options_hover = Some(id);
        self.options_acted = None;
        true
    }

    /// Which side of a control the rest of its group sits on, from the resting
    /// layout it just left. Ties (and a group of one) open toward the middle of
    /// the bar, which is always where more room is.
    fn group_lies_left_of(&self, id: PillId, rect: Rect) -> bool {
        let g = group_of(id);
        let mut left = 0;
        let mut right = 0;
        for p in self.options_pills_resting() {
            if p.id == id || group_of(p.id) != g {
                continue;
            }
            if p.rect.x < rect.x {
                left += 1;
            } else {
                right += 1;
            }
        }
        match left.cmp(&right) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => rect.x > self.options_size.0 as f32 / 2.0,
        }
    }

    /// Drop the sticky pair once the pointer has been off it for the shared
    /// leave-hold. Not a countdown on the feature — it answers to the pointer
    /// (§4) and borrows §3's transit grace so that moving between the control
    /// and its doorway, or over the gap between them, never counts as leaving.
    fn schedule_sticky_drop(&mut self) {
        let timer = Timer::from_duration(animation::LEAVE_HOLD);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                // Still standing, and the hand has not come back to it.
                if app.options_sticky.is_some() && app.options_hover.is_none() {
                    app.clear_sticky();
                }
                TimeoutAction::Drop
            });
    }

    /// Put the focused window into a mode, leaving whatever mode it was in.
    ///
    /// Handles the one transition the compositor cannot do in a single step:
    /// entering pseudo from fullscreen or floating, where the Golem pseudo size
    /// is a fraction of the window's TILE and the tile is not measurable until
    /// the window is back in the layout. There the first pass returns it to the
    /// layout and this schedules the second, once the move has settled.
    /// Ask for a [`Self::sync_solitary_pseudo`] once the layout has settled.
    ///
    /// Deferred because the rule needs the tile it is about to take a fraction
    /// of, and right after a window opens or closes the compositor is still
    /// reporting the rectangle it is animating *through* — sizing off that
    /// pseudotiles to a fraction of a shape the desktop was only passing
    /// through (the same trap `set_window_mode`'s second pass exists for).
    ///
    /// Coalesced: closing an app that takes three windows with it fires three
    /// events and runs one sweep.
    pub(crate) fn schedule_solitary_pseudo(&mut self) {
        if self.pseudo_sweep_pending {
            return;
        }
        self.pseudo_sweep_pending = true;
        let timer = Timer::from_duration(LAYOUT_SETTLE);
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.pseudo_sweep_pending = false;
                app.sync_solitary_pseudo();
                TimeoutAction::Drop
            });
    }

    /// Golem's layout rule: **a space showing one tile shows it pseudo.**
    ///
    /// A lone window stretched across a whole screen is a shape nobody chose —
    /// it is just what "one window, tiled" happens to produce. So Golem gives
    /// it its own proportions instead (Max, 2026-09-13), and takes them back the
    /// moment the space has to be shared: open a second window and both go plain
    /// tiled, close back down to one and the survivor becomes pseudo again.
    ///
    /// It runs on layout EVENTS — a window opening, closing, moving — never
    /// continuously, and that is what leaves room for the bar's own controls: a
    /// mode you set by hand stands until the space changes shape again. The
    /// stage is skipped entirely; there the mode owns every window's geometry.
    ///
    /// Floating and fullscreen windows are not tiles ([`hypr::LayoutWindow::is_tile`]),
    /// so neither counts toward "how many", and neither is ever moved by this.
    pub(crate) fn sync_solitary_pseudo(&mut self) {
        // A tiling rule: skipped while Golem is a FLOATING window manager, where
        // there are no tiles for it to judge (`settings.rs`), and on the stage,
        // where the mode owns every window's geometry.
        if self.stage.is_on() || self.floating_mode() {
            return;
        }
        let windows = hypr::layout_windows();
        let mut spaces: std::collections::BTreeMap<i64, Vec<&hypr::LayoutWindow>> =
            std::collections::BTreeMap::new();
        for w in windows.iter().filter(|w| w.is_tile()) {
            spaces.entry(w.workspace).or_default().push(w);
        }
        for (ws, tiles) in spaces {
            match tiles.as_slice() {
                // The one tile on the space: it gets Golem's proportions. The
                // rectangle it is a fraction of is computed, not measured (see
                // `hypr::solitary_tile`) — measuring would mean waiting for the
                // window to finish arriving first.
                // Re-asserted even when it is ALREADY pseudo, because the size
                // can be stale: a pseudo window that floats and comes back
                // keeps whatever shape the round trip left it (measured —
                // `994×304` where the rule promises `1780×1026`). Golem's
                // pseudo is a fixed fraction of the tile, so writing it again
                // is idempotent where it is already right.
                [only] => {
                    if only.mode == hypr::WindowMode::Tiled || only.mode == hypr::WindowMode::Pseudo
                    {
                        if let Some(tile) = hypr::solitary_tile() {
                            tracing::debug!("layout: ws{ws} is down to one tile — pseudo");
                            hypr::pseudo_on(&only.address, tile);
                        }
                    }
                }
                // Shared: the space is divided, so nothing is pseudo and the
                // user is left to ask for whatever they want instead.
                many => {
                    for w in many.iter().filter(|w| w.mode == hypr::WindowMode::Pseudo) {
                        tracing::debug!("layout: ws{ws} shares {} tiles — plain", many.len());
                        hypr::pseudo_off(&w.address);
                    }
                }
            }
        }
        // The bar says what the focused window is, and this may have just
        // changed it.
        self.refresh_window_mode();
        // And now the dock may judge the layout: `on_layout_changed` holds off
        // while a sweep is pending, precisely so it never sees the full-size
        // tile a window only passes through on its way to being pseudo.
        self.on_layout_changed();
    }

    /// Re-read what the focused window IS, for the state pill.
    ///
    /// One `j/activewindow` read, and only at the moments the answer can have
    /// changed: focus moved, or a mode was just set.
    pub(crate) fn refresh_window_mode(&mut self) {
        let (addr, mode) = match hypr::active_window_mode() {
            Some(read) => (Some(read.0), read.1),
            None => (None, hypr::WindowMode::Tiled),
        };
        // Whether this is the same window we last read, which is what makes a
        // change a *transition* rather than just a different window's shape.
        let same = addr == self.options_mode_addr;
        if mode == self.options_mode && same {
            return;
        }
        let was_floating = self.options_mode == hypr::WindowMode::Floating;
        self.options_mode = mode;
        self.options_mode_addr = addr;
        // A window joining or leaving the LAYOUT changes how many tiles its
        // space is divided between, and no compositor event says so (neither
        // the socket nor the internal bus carries one — checked both), so this
        // read is where the solitary-pseudo rule hears about it. Without it,
        // un-floating onto an otherwise empty space left the window plainly
        // tiled: the one case the rule promises and could not see.
        //
        // Only floating, and only on the same window. Fullscreen is not a
        // trigger — a window going full-screen and coming back is the same
        // window on the same space throughout, and sweeping there re-imposed
        // pseudo on one the user had deliberately set plain (Max, 2026-09-13).
        if same && (mode == hypr::WindowMode::Floating) != was_floating {
            self.schedule_solitary_pseudo();
        }
        self.sync_window_state();
    }

    /// Let the state pill catch up with the window — but not under the hand.
    ///
    /// "The Still Bar" (`OptionUXRules.md` §2). The mode the cluster is laid out
    /// from decides which toggle is missing from the row, so adopting a change
    /// immediately re-flows the buttons **at the moment of the click that caused
    /// it**: press [float] and float leaves the row, the state pill takes its
    /// place, and everything right of it slides — so pressing it again to turn
    /// floating back off means first hunting for where it went (Max,
    /// 2026-09-12).
    ///
    /// So the layout keeps the mode it had while the hand is on the bar, and
    /// the re-flow waits at the door. The window itself moves at once — the
    /// feedback is on the desktop, which is where the change actually is — and
    /// the buttons stay exactly where they were aimed at.
    ///
    /// Every button on the frozen row therefore acts on **what it shows**,
    /// including the state pill: that is what makes it a toggle you can press
    /// twice without moving.
    pub(crate) fn sync_window_state(&mut self) {
        if self.options_mode_shown == self.options_mode || self.options_ptr_on_bar() {
            return;
        }
        self.options_mode_shown = self.options_mode;
        self.sync_options_input();
        self.draw_options();
    }

    pub(crate) fn set_window_mode(&mut self, target: hypr::WindowMode) {
        if !hypr::set_window_mode(target) {
            // Refused (already in the target, nothing focused): the cached mode
            // can still be stale — a client may have gone fullscreen by itself.
            self.refresh_window_mode();
            return;
        }
        self.refresh_window_mode();
        // Long enough for the window to reach its tile: the compositor reports
        // a mid-ANIMATION size until it lands (see `docs/hypr-api.md`), and a
        // size read too early would pseudotile to a fraction of a rectangle the
        // window was only passing through.
        let timer = Timer::from_duration(MODE_SETTLE);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                hypr::set_window_mode(target);
                // The second pass can land somewhere the first did not (pseudo from
                // fullscreen needs the tile it only has once back in the layout), so
                // the word is read again rather than assumed.
                app.refresh_window_mode();
                TimeoutAction::Drop
            });
    }

    /// Show or conceal the bar. The one door every path goes through — the
    /// top-edge dwell, a fullscreen taking the screen, the pointer leaving, a
    /// doorway opening — so the surface has exactly one entrance and one exit,
    /// both on the shared tempo (`OptionUXRules.md` §3).
    ///
    /// The flag is the intent and flips at once; `options_show` is the drawn
    /// presence and catches up. Input follows the intent, so a bar on its way
    /// out is not clickable and one on its way in already is.
    pub(crate) fn set_options_hidden(&mut self, hidden: bool) {
        if self.options_hidden == hidden {
            return;
        }
        self.options_hidden = hidden;
        if animation::reduce_motion() {
            self.options_show.t = if hidden { 0.0 } else { 1.0 };
            return;
        }
        self.schedule_options_show_frame();
    }

    fn schedule_options_show_frame(&mut self) {
        if self.options_show.frame_pending {
            return;
        }
        self.options_show.frame_pending = true;
        if self.options_show.last.is_none() {
            self.options_show.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_show.frame_pending = false;
                app.tick_options_show();
                TimeoutAction::Drop
            });
    }

    /// One frame of the bar's fade.
    fn tick_options_show(&mut self) {
        let now = Instant::now();
        let dt = self
            .options_show
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.options_show.last = Some(now);
        let target = if self.options_hidden { 0.0 } else { 1.0 };
        let (t, moving) = ease_toward(
            self.options_show.t,
            target,
            dt,
            animation::MORPH_RATE,
            animation::SETTLE_ALPHA,
        );
        self.options_show.t = t;
        self.draw_options();
        if moving {
            self.schedule_options_show_frame();
        } else {
            self.options_show.last = None;
            // The bar is all the way back, so the pair it came back around has
            // finished handing over: the doorway's slot is an ordinary OPTION
            // again (`OptionUXRules.md` §4).
            if t >= 1.0 && self.options_sticky.is_some() {
                self.options_sticky = None;
                self.options_update_hover();
                self.draw_options();
            }
        }
    }

    /// How present a given pill should be drawn right now.
    ///
    /// The bar fades as one, except for the sticky pair: the whole point of
    /// those two is that they do NOT go, so the rest fades around them — out
    /// when the bar is taken away, and back in when a doorway brings it back
    /// (`OptionUXRules.md` §4).
    fn options_pill_fade(&self, id: PillId, rect: Rect) -> f32 {
        if let Some(st) = self.options_sticky.as_ref() {
            let in_doorway = rect.x < st.door.x + st.door.w && st.door.x < rect.x + rect.w;
            if id == st.id || id == PillId::Doorway || in_doorway {
                return 1.0;
            }
        }
        self.options_show.t
    }

    /// Debug/verification only: conceal the bar and stand `[fullscreen]` plus
    /// its doorway on it, as a real fullscreen click would. The §4 pair is
    /// otherwise reachable only with a pointer on a real fullscreen window,
    /// which no screenshot can arrange.
    pub(crate) fn debug_stand_sticky(&mut self) {
        let Some(full) = self
            .options_pills_resting()
            .into_iter()
            .find(|p| p.id == PillId::Fullscreen)
        else {
            return;
        };
        self.options_acted = Some((full.id, full.rect, Instant::now()));
        self.set_options_hidden(true);
        self.options_ptr = Some((full.rect.x + full.rect.w / 2.0, full.rect.y));
        self.arm_sticky();
        self.options_update_hover();
        self.sync_options_input();
        self.draw_options();
    }

    /// Take the sticky pair down. The bar underneath is unchanged — it was
    /// already concealed — so this simply stops drawing a way back that is no
    /// longer wanted or no longer true.
    fn clear_sticky(&mut self) {
        if self.options_sticky.take().is_none() {
            return;
        }
        self.options_acted = None;
        self.sync_options_input();
        // Hover is RE-DERIVED from where the pointer actually is, never
        // dropped. The hand did not move — the world did — so whatever now
        // sits under it is hovered, and a toggle can be repeated as many times
        // as wanted without re-aiming between presses (`OptionUXRules.md` §1's
        // repeatability, carried across the bar going away and coming back).
        // If the pointer really has left, this resolves to nothing anyway.
        //
        // The Leader is deliberately NOT snapped: it was pinned to this control
        // when it was clicked, and zeroing that displacement would re-flow the
        // cluster under a stationary finger — the exact failure §1 exists to
        // prevent, arriving at the worst possible moment.
        self.options_update_hover();
        self.draw_options();
    }

    /// The doorway was hovered: bring the whole bar back, here and now. No
    /// dwell — the pointer is on it deliberately, and the bar was never really
    /// left (`OptionUXRules.md` §4). The real OPTION lands in the doorway's
    /// exact slot, so it reads as that pill arriving, not as a swap.
    fn open_doorway(&mut self) {
        let Some(st) = self.options_sticky.take() else {
            return;
        };
        // "The OPTIONS as they were when you pressed the sticky one." The
        // chain is restored to the exact progress it was frozen at, with its
        // stagger clock cleared so no stage waits its turn again: the bar is
        // simply back, mid-state and all. Nothing replays, nothing rebuilds.
        self.options_ctrl.t = st.ctrl;
        self.options_ctrl.reveal = st.ctrl_reveal;
        self.options_ctrl.changed_at = None;
        self.options_ctrl.last = None;
        self.options_acted = None;
        self.set_options_hidden(false);
        self.options_reveal_deadline = None;
        // The Leader is deliberately NOT snapped: the control was pinned when
        // it was clicked (§1), and that anchor is part of "as they were".
        self.sync_options_input();
        self.options_update_hover();
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
                        app.set_options_hidden(false);
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
                        app.set_options_hidden(true);
                        app.options_hover = None;
                        // The bar is going away — nothing to ease home to.
                        app.snap_lead();
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
        // A sticky pair sits on an otherwise-concealed bar: the pointer has to
        // reach the pair, and the top-edge reveal strip has to keep working
        // underneath it, so both are input (`OptionUXRules.md` §4).
        if self.options_hidden {
            if let Some(st) = self.options_sticky.as_ref() {
                let l = st.rect.x.min(st.door.x);
                let r = (st.rect.x + st.rect.w).max(st.door.x + st.door.w);
                let strip = (0, 0, w as i32, REVEAL_PX.ceil() as i32);
                let pair = (
                    l.floor() as i32,
                    0,
                    (r - l).ceil() as i32,
                    (st.rect.y + st.rect.h).ceil() as i32,
                );
                surface::set_input_rects(&self.compositor, layer, &[strip, pair]);
                return;
            }
        }
        let h = if self.options_hidden {
            REVEAL_PX.ceil() as i32
        } else if self.notif.expanded
            || self.clip.expanded
            || self.module_box_open
            || self.play_box_open
            || self.stats.open
        {
            // Extend the pointer-sensitive region down over whichever box is
            // open so scroll/hover/clicks there reach us instead of passing
            // through. Use the *fully-expanded* bottom (not the live animating
            // height) so the region is stable the instant a box opens.
            let mut bottom = self.options_bar_h();
            if self.notif.expanded {
                bottom = bottom.max(self.notif_input_bottom());
            }
            if self.clip.expanded {
                bottom = bottom.max(self.clip_input_bottom());
            }
            if self.play_box_open {
                bottom = bottom.max(self.play_box_input_bottom());
            }
            if self.module_box_open {
                bottom = bottom.max(self.module_box_input_bottom());
            }
            if self.stats.open {
                bottom = bottom.max(self.stats_box_input_bottom());
            }
            bottom.ceil() as i32
        } else {
            self.options_bar_h().ceil() as i32
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
        } else if self
            .deck_layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            PointerSurface::Deck
        } else {
            PointerSurface::Dock
        }
    }

    /// Route a pointer event that belongs to the STAGE deck: hover tracks the
    /// tile under the pointer, a left release puts that task on the stage.
    pub(crate) fn deck_pointer(&mut self, event: wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => self.deck_motion(surface_x as f32, surface_y as f32),
            wl_pointer::Event::Leave { .. } => {
                self.pointer_surface = PointerSurface::Dock;
                if self.deck.hover.take().is_some() {
                    self.draw_deck();
                }
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(wl_pointer::ButtonState::Released),
                ..
            } if button == BTN_LEFT => {
                if let Some((x, y)) = self.deck_ptr {
                    self.deck_click(x, y);
                }
            }
            _ => {}
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
                // Pointer off the bar: the Leader is released and its group
                // eases home to its resting position (`OptionUXRules.md` §1).
                // Nothing is permanently displaced.
                self.release_leader();
                self.options_ptr = None;
                self.pointer_surface = PointerSurface::Dock;
                self.options_reveal_deadline = None;
                // A sticky pair lives on a CONCEALED bar, so the cleanup below
                // (guarded on the bar being up) never runs for it. Walking away
                // is one of its three endings (`OptionUXRules.md` §4), so it is
                // handled here: hover goes, and the pair goes after the shared
                // transit grace unless the hand comes back.
                if self.options_sticky.is_some() {
                    self.options_hover = None;
                    self.schedule_sticky_drop();
                    self.draw_options();
                }
                // A sunset prompt that arrived (or resolved) while the hand
                // was on the bar plays its held morph now (`§2`). So does a
                // window-mode change made from these very buttons: the row
                // re-flows at the door, never under the hand that pressed it.
                self.sync_module();
                self.sync_window_state();
                if !self.options_hidden {
                    self.options_hover = None;
                    self.update_ctrl_reveal(); // fade the buttons out
                    self.update_clock_meta(); // start the date's hold-then-collapse
                    self.update_notif_reveal(); // collapse the bell's peek/history
                    self.update_clip_reveal(); // collapse the clipboard's peek
                    self.update_stats_reveal(); // and the gear's stats child
                                               // The playing box ends with the visit, like every other box
                                               // on this bar — it stays for as long as the hand is on the
                                               // surface (§2) and folds away when it leaves.
                    self.play_box_open = false;
                    self.update_cava_reveal(); // tuck the cava children back
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
                    && self.options_interactive() =>
            {
                // The Leader (`OptionUXRules.md` §1): the click pins the pill it
                // lands on, so the re-flow the click causes happens around it.
                // On PRESS — the anchor is the layout that was aimed at.
                self.capture_leader();
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_LEFT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released)
                    && self.options_interactive() =>
            {
                self.options_click();
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_RIGHT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released)
                    && self.options_interactive() =>
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
            // Scroll over the gear or its readout: down grows the panel, up
            // folds it back. The same gesture the clipboard and the bell answer
            // to, at the other end of the same bar.
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } if !self.options_hidden
                && (matches!(
                    self.options_hover,
                    Some(PillId::Settings | PillId::SettingsStats)
                ) || self.stats.open) =>
            {
                self.stats_axis(value as f32);
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
            // Scroll over the track name: open the playing box, or close it.
            //
            // The sign matches the volume axis on the transport pill next door,
            // which was flipped for the same reason — this session's scroll
            // convention is the opposite of the raw Wayland sign, and two
            // gestures an inch apart disagreeing about which way is "down"
            // would be worse than either choice.
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } if !self.options_hidden
                && (self.options_hover == Some(PillId::CavaNow) || self.play_box_open) =>
            {
                // **A scroll's END arrives as an axis event carrying ZERO**, and
                // a kinetic flick trails a run of ever-smaller ones. Reading the
                // sign of those closed the box the instant the finger stopped —
                // it looked like a spring and was really the stop event voting
                // "up". Only a real push counts.
                let v = value as f32;
                if v.abs() < SCROLL_DEADZONE {
                    return;
                }
                let open = v < 0.0;
                if open != self.play_box_open {
                    self.play_box_open = open;
                    if open {
                        // Only decode art for a panel someone is opening.
                        self.request_play_art();
                    }
                    self.schedule_cava_frame();
                    self.sync_options_input();
                    self.draw_options();
                }
            }
            // Scroll over the transport pill: volume on the vertical axis,
            // tracks on the horizontal one. The pill and its three symbols
            // only — the track name and the output do not take gestures.
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } if !self.options_hidden
                && self.options_hover.is_some_and(|h| h.is_cava_transport()) =>
            {
                self.cava_axis(axis, value as f32);
            }
            _ => {}
        }
    }

    /// A scroll over the cava cluster.
    ///
    /// Both axes accumulate rather than acting per event: a wheel notch arrives
    /// as several small deltas and a touchpad as a stream of tiny ones, so
    /// acting on each would skip six tracks for one flick. The accumulator is
    /// reset when the direction reverses, so a change of mind is immediate
    /// rather than having to pay off the distance already travelled.
    ///
    /// The two thresholds differ on purpose. Volume is cheap and continuous —
    /// overshooting costs a nudge back. Skipping a track is discrete and
    /// expensive: the thing you were listening to is gone, and `[prev]` on many
    /// players restarts rather than returns. So tracks demand a deliberate
    /// push, roughly one firm notch.
    fn cava_axis(&mut self, axis: wl_pointer::Axis, value: f32) {
        const VOL_STEP: f32 = 6.0;
        const TRACK_STEP: f32 = 14.0;
        match axis {
            wl_pointer::Axis::VerticalScroll => {
                if self.cava_scroll_accum_v * value < 0.0 {
                    self.cava_scroll_accum_v = 0.0;
                }
                self.cava_scroll_accum_v += value;
                while self.cava_scroll_accum_v.abs() >= VOL_STEP {
                    let up = self.cava_scroll_accum_v > 0.0;
                    self.cava_scroll_accum_v -= VOL_STEP * self.cava_scroll_accum_v.signum();
                    self.cava_volume_step(up);
                }
            }
            wl_pointer::Axis::HorizontalScroll => {
                if self.cava_scroll_accum_h * value < 0.0 {
                    self.cava_scroll_accum_h = 0.0;
                }
                self.cava_scroll_accum_h += value;
                while self.cava_scroll_accum_h.abs() >= TRACK_STEP {
                    let forward = self.cava_scroll_accum_h > 0.0;
                    self.cava_scroll_accum_h -= TRACK_STEP * self.cava_scroll_accum_h.signum();
                    self.cava_transport(if forward { "next" } else { "previous" });
                }
            }
            _ => {}
        }
    }

    /// Shared Enter/Motion logic: reveal-dwell at the top edge while concealed,
    /// otherwise hover the pills and cancel any pending conceal.
    fn options_on_motion(&mut self, y: f32) {
        if self.options_hidden {
            // A sticky pair is live on the concealed bar: it hovers and clicks
            // like any other pill (`OptionUXRules.md` §4). The top-edge dwell
            // still runs underneath, so the ordinary way back keeps working.
            if self.options_sticky.is_some() {
                self.options_update_hover();
            }
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
        let bar_h = self.options_bar_h();
        // One layout, shared by the hit-test and the Leader capture — so the
        // grab is measured against exactly the rects the pointer is over.
        let pills = self.options_pills();
        let hover = self.options_ptr.and_then(|p| {
            pills
                .iter()
                .find(|pill| {
                    // The notification element owns its whole (possibly tall,
                    // below-the-bar) rect so hover holds while its history is
                    // open; every other pill gets the full-bar-height hit (up to
                    // the top screen edge) so slamming to the edge still lands.
                    // The elements that own a rect BELOW the bar keep hover for
                    // its whole height, so the pointer inside an open box still
                    // counts as being on the thing it came out of. The track
                    // pill joined them when it learned to grow into the playing
                    // list, and the gear's readout when it grew a panel —
                    // without that, moving into its own box reads as LEAVING it
                    // and the box folds under the hand (Max, 2026-09-13: *"the
                    // box closes even when im hovering it"*). Anything here that
                    // grows downward belongs on this list.
                    let hit = if matches!(
                        pill.id,
                        PillId::Notif
                            | PillId::ClipboardBox
                            | PillId::CavaNow
                            | PillId::SettingsStats
                    ) {
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
        // Sticky OPTIONS (`OptionUXRules.md` §4). Hovering the doorway brings
        // the bar back at once; leaving the pair takes it down after the shared
        // transit grace, so crossing the gap between the two is not leaving.
        if self.options_sticky.is_some() {
            if hover == Some(PillId::Doorway) {
                self.open_doorway();
                return;
            }
            if hover.is_none() {
                self.schedule_sticky_drop();
            }
        }
        // The Leader (`OptionUXRules.md` §1) is chosen by the click, not by the
        // hover — travelling across the bar must not move anything. The anchor
        // holds until the pointer leaves the bar strip, which includes going
        // down into an open box.
        if self.options_leader.is_some() && !self.options_ptr_on_bar() {
            self.release_leader();
        }
        self.update_ctrl_reveal();
        self.update_clock_meta();
        self.update_notif_reveal();
        self.update_clip_reveal();
        self.update_stats_reveal();
        self.update_cava_reveal();
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
        // A sticky pair is the whole layout: both its pills are up by
        // definition, whatever the reveal chain's progress happens to be
        // (`OptionUXRules.md` §4).
        if self.options_sticky.is_some() {
            return true;
        }
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

    /// The bar's pill height — pills fill almost the whole bar, top to bottom.
    /// The one place the formula lives; the layout and every morph span that
    /// counts in pills read it from here.
    pub(crate) fn options_pill_h(&self) -> f32 {
        (self.options_bar_h() - 2.0 * PILL_MARGIN_Y).max(1.0)
    }

    /// How far a mode toggle slides as its progress runs 0→1: out from behind
    /// its parent's near edge to its resting spot, i.e. one pill plus the gap
    /// it emerges across. The span [`animation::settle_t`] measures against.
    fn options_ctrl_travel(&self) -> f32 {
        self.options_pill_h() + GROUP_GAP
    }

    /// Whether the surface takes clicks right now: the bar is up, or it is
    /// concealed but a sticky OPTION is standing on it, which is the whole
    /// point of it standing there (`OptionUXRules.md` §4).
    fn options_interactive(&self) -> bool {
        !self.options_hidden || self.options_sticky.is_some()
    }

    /// Whether the pointer is on the bar strip itself — not merely somewhere on
    /// the OPTIONS surface, which extends down over an open box. This is what
    /// "until the pointer leaves the bar" measures, gaps between pills included.
    fn options_ptr_on_bar(&self) -> bool {
        self.options_ptr
            .is_some_and(|(_, y)| (0.0..=self.options_bar_h()).contains(&y))
    }

    /// Whether the pointer is within the window+controls cluster span (so a
    /// small gap between pills doesn't count as leaving).
    fn options_ptr_in_cluster(&self) -> bool {
        let Some((x, y)) = self.options_ptr else {
            return false;
        };
        if y < 0.0 || y > self.options_bar_h() {
            return false;
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &self.options_pills() {
            if matches!(p.id, PillId::Window | PillId::Close | PillId::WindowState)
                || ctrl_index(p.id).is_some()
            {
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
    /// Capture the Leader: the pill being clicked, pinned to the place on the
    /// bar it occupies right now. Taken on PRESS, before the action runs, so
    /// the anchor is the layout the user actually aimed at.
    ///
    /// The anchor is read from the DRAWN rect, so clicking a second pill while
    /// its group is already displaced hands leadership over without a jump.
    ///
    /// A click between pills, or on a group the rule does not cover, leaves the
    /// standing leader alone: you do not lose your grip by missing, and merely
    /// travelling across the bar changes nothing at all.
    fn capture_leader(&mut self) {
        let drawn = self.options_pills();
        let Some(id) = self
            .options_hover
            .filter(|id| group_slot(group_of(*id)).is_some())
            // The sunset module (the Window group while the prompt is shown) is
            // a fixed panel that owns its own morph — it must NOT slide under
            // the pointer like the window-title cluster (§1). Clicking its gear
            // pinned the Window group as leader and shifted the glass panel off
            // its own content. Exclude it from leading entirely.
            .filter(|id| !(self.module_on_pill() && group_of(*id) == PillGroup::Window))
            // The cava cluster shares the Mind group but is nothing like the
            // Mind's ranked row: it is anchored to the clipboard, never
            // re-ranks, and does not re-lay-out when used. §1 exists for groups
            // that move under your hand as you act on them; pinning one that
            // does not can only DISPLACE it — which is exactly what pressing a
            // transport symbol did, shifting the cluster out from under the
            // click between press and release (2026-09-12).
            .filter(|id| !id.is_cava())
        else {
            return;
        };
        let Some(rect) = drawn.iter().find(|p| p.id == id).map(|p| p.rect) else {
            return;
        };
        // A Mind control is held by its ACTION, not its slot.
        let action = match id {
            PillId::Option(i) => match self
                .surfaced_options()
                .get(i as usize)
                .filter(|_| (i as usize) < OPTION_PILL_CAP)
            {
                Some(a) => Some(a.id),
                None => return,
            },
            _ => None,
        };
        self.options_leader = Some(Leader {
            id,
            action,
            anchor_cx: rect.x + rect.w / 2.0,
        });
    }

    /// Keep the stored displacement in step with the live one, and release the
    /// leader when the pill it holds leaves the layout — a window closed, a
    /// Mind offer withdrawn, a toggle tucked away. The stored value is the last
    /// good one, so the group eases home from where it actually is rather than
    /// snapping. Runs before anything consumes the layout.
    pub(crate) fn sync_lead(&mut self) {
        let held = if self.options_leader.is_some() {
            let resting = self.options_pills_resting();
            match self.live_lead(&resting) {
                Some((slot, live)) => {
                    self.options_lead.off[slot] = live;
                    Some(slot)
                }
                // The leader is gone: keep whatever offset it had and go home.
                None => {
                    self.options_leader = None;
                    None
                }
            }
        } else {
            None
        };
        // Every group that is NOT the held one eases home — including the group
        // the leader just left, when a click hands leadership from one OPTION
        // to another.
        if (0..LEAD_N).any(|s| held != Some(s) && self.options_lead.off[s] != 0.0) {
            self.schedule_options_lead_frame();
        }
    }

    /// Release the Leader and ease its group home to the resting layout.
    /// Nothing is permanently displaced.
    fn release_leader(&mut self) {
        // Holding still is not an animation, but the ease home is — so
        // reduce-motion snaps it, and a bar that is going away has nothing to
        // ease home on.
        if self.options_hidden || animation::reduce_motion() {
            return self.snap_lead();
        }
        self.sync_lead(); // freeze the last live offset
        self.options_leader = None;
        // Idle at rest: a leave that never displaced anything costs no frames.
        if self.options_lead.off.iter().any(|o| *o != 0.0) {
            self.schedule_options_lead_frame();
        }
    }

    /// Drop the Leader and any displacement outright — the bar is going away,
    /// so there is nothing to ease home to.
    pub(crate) fn snap_lead(&mut self) {
        self.options_leader = None;
        self.options_lead.off = [0.0; LEAD_N];
    }

    fn schedule_options_lead_frame(&mut self) {
        if self.options_lead.frame_pending {
            return;
        }
        self.options_lead.frame_pending = true;
        if self.options_lead.last.is_none() {
            self.options_lead.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_lead.frame_pending = false;
                app.tick_options_lead();
                TimeoutAction::Drop
            });
    }

    /// One frame of the ease home. The held group (if any) is skipped — its
    /// offset is derived, not decayed.
    fn tick_options_lead(&mut self) {
        let now = Instant::now();
        let dt = self
            .options_lead
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.options_lead.last = Some(now);
        let held = self
            .leader_pill_id()
            .and_then(|id| group_slot(group_of(id)));
        let mut active = false;
        for slot in 0..LEAD_N {
            if held == Some(slot) {
                continue;
            }
            // Already a real distance in logical px, so it settles straight on
            // the shared threshold (`OptionUXRules.md` §3).
            let (n, moving) = ease_toward(
                self.options_lead.off[slot],
                0.0,
                dt,
                animation::MORPH_RATE,
                animation::SETTLE_PX,
            );
            self.options_lead.off[slot] = n;
            active |= moving;
        }
        self.draw_options();
        if active {
            self.schedule_options_lead_frame();
        } else {
            self.options_lead.last = None;
        }
    }

    /// Start a title metamorphosis: the pill eases from the width it is showing
    /// now to the freshly measured one, crossfading the two names. Under the
    /// Leader the pill's leader-facing edge is pinned, so the whole ease plays
    /// out on its far side.
    fn begin_title_morph(&mut self, from: f32, outgoing: String) {
        // A live-resize readout retitles every 40ms — that is a counter, not a
        // metamorphosis. Reduce-motion goes straight to the answer.
        if animation::reduce_motion() || self.options_resize_live.is_some() {
            self.options_title_meta.t = 1.0;
            return;
        }
        self.options_title_meta.from = from;
        self.options_title_meta.outgoing = outgoing;
        self.options_title_meta.t = 0.0;
        self.options_title_meta.last = None;
        self.schedule_options_title_frame();
    }

    fn schedule_options_title_frame(&mut self) {
        if self.options_title_meta.frame_pending {
            return;
        }
        self.options_title_meta.frame_pending = true;
        if self.options_title_meta.last.is_none() {
            self.options_title_meta.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_title_meta.frame_pending = false;
                app.tick_options_title();
                TimeoutAction::Drop
            });
    }

    fn tick_options_title(&mut self) {
        let now = Instant::now();
        let dt = self
            .options_title_meta
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.options_title_meta.last = Some(now);
        // The progress carries the width change between the two titles, so the
        // settle is measured against that span (`OptionUXRules.md` §3).
        let (t, moving) = ease_toward(
            self.options_title_meta.t,
            1.0,
            dt,
            animation::MORPH_RATE,
            animation::settle_t(self.options_title_w - self.options_title_meta.from),
        );
        self.options_title_meta.t = t;
        self.draw_options();
        if moving {
            self.schedule_options_title_frame();
        } else {
            self.options_title_meta.last = None;
            self.options_title_meta.outgoing.clear();
            self.options_title_meta.outgoing_module = None;
        }
    }

    fn update_ctrl_reveal(&mut self) {
        // While a sticky OPTION stands, the chain is FROZEN at the state the
        // bar went away in (`OptionUXRules.md` §4). Otherwise moving onto the
        // doorway — which is outside the cluster span, the cluster being one
        // pill wide now — would retract the whole chain, and the bar the
        // doorway brings back would be a bar mid-rebuild instead of the one
        // that was left.
        if self.options_sticky.is_some() {
            return;
        }
        let want = if self.options_ctrl.reveal {
            self.options_ptr_in_cluster()
        } else {
            matches!(
                self.options_hover,
                Some(PillId::Window) | Some(PillId::Close) | Some(PillId::WindowState)
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
            // The progress carries a slide of one pill plus the gap it emerges
            // across (`origin` → rest in `draw_options`), so that is the span
            // its settle is measured in (`OptionUXRules.md` §3).
            let (nt, moving) = ease_toward(
                self.options_ctrl.t[i],
                target,
                dt,
                animation::MORPH_RATE,
                animation::settle_t(self.options_ctrl_travel()),
            );
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
    /// hovered; once the pointer leaves the SURFACE it holds on the date for
    /// [`animation::LEAVE_HOLD`] then collapses back to the clock (same transition
    /// backwards).
    ///
    /// The collapse waits for the pointer to leave rather than firing when the
    /// clock itself is left, because the date's ~180px shrink hands the bell
    /// back ([`clear_under`]): a pill the user may be travelling toward would
    /// otherwise reappear under them, on a timer, for a reason they did not ask
    /// for — and a change nobody requested waits until the visit is over
    /// (`OptionUXRules.md` §2, "The Still Bar"). It used to be worse: the
    /// notification cluster was pinned to the clock's LIVE edge, so growing the
    /// date shoved the whole right end of the bar sideways. It now grows over
    /// it instead (see [`Self::options_clock_rest_left`]), and this hold
    /// guards only the hand-back.
    fn update_clock_meta(&mut self) {
        // An OPEN notification drawer owns this corner: the clock may take the
        // bell's place, but not a box's. Without this the date would grow ~180px
        // across the drawer's top band — and the drawer, which draws itself and
        // `continue`s, would print its first card's time straight back through
        // the date. The clock is never removed for it (nothing may take the
        // clock away); it simply stays the time until the box closes.
        if self.notif.occludes_below_bar() {
            if self.options_clock_meta.reveal {
                self.options_clock_meta.reveal = false;
                self.options_clock_meta.last = None;
                self.schedule_options_clock_frame();
            }
            return;
        }
        if self.options_hover == Some(PillId::Clock) {
            // Hovering the clock: reveal, and cancel any pending collapse.
            self.options_clock_meta.hold_deadline = None;
            if !self.options_clock_meta.reveal {
                self.options_clock_meta.reveal = true;
                self.options_clock_meta.last = None;
                self.schedule_options_clock_frame();
            }
        } else if clock_may_collapse(
            self.options_clock_meta.reveal,
            self.options_clock_meta.hold_deadline.is_some(),
            self.options_hover == Some(PillId::Clock),
        ) {
            // Left the PILL while showing the date: hold, then collapse.
            self.schedule_clock_collapse();
        }
    }

    /// After the pointer leaves THE CLOCK, keep the date up for the shared
    /// [`animation::LEAVE_HOLD`], then play the metamorphosis backwards —
    /// unless the pointer came back onto the pill inside the hold, in which
    /// case the collapse is abandoned rather than played under them (it is
    /// re-armed by the next leave). The hold is what keeps a pointer merely
    /// crossing the clock from playing the whole morph out and back; see
    /// [`clock_may_collapse`] for why leaving the pill is now enough.
    fn schedule_clock_collapse(&mut self) {
        let deadline = Instant::now() + animation::LEAVE_HOLD;
        self.options_clock_meta.hold_deadline = Some(deadline);
        let timer = Timer::from_duration(animation::LEAVE_HOLD);
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
        // The progress carries the whole clock→date width change, so the settle
        // is measured against that span (`OptionUXRules.md` §3).
        let (nt, moving) = ease_toward(
            self.options_clock_meta.t,
            target,
            dt,
            animation::MORPH_RATE,
            animation::settle_t(self.options_date_w - self.options_clock_w),
        );
        self.options_clock_meta.t = nt;
        self.draw_options();
        if moving {
            self.schedule_options_clock_frame();
        } else {
            self.options_clock_meta.last = None;
        }
    }

    // --- Sunset prompt (see the "Sunset prompt" constants above) ------------

    /// The Mind's live sunset offer, if any. Read from the RAW option set:
    /// the offer is deliberately not "surfaced" as a cluster pill
    /// ([`is_surfaced_affordance`] excludes it) — this prompt is its surface.
    fn sunset_offer(&self) -> Option<&options_engine::Affordance> {
        self.options.items.iter().find(|a| a.id == SUNSET_OFFER_ID)
    }

    /// Whether the prompt WANTS the pill: offered (or debug-forced) and not
    /// already answered by the user within this offer-cycle.
    fn sunset_prompt_wanted(&self) -> bool {
        !self.sunset_acted
            && (self.module_debug == Some(Module::Sunset)
                || self.sunset_recalled
                || self.sunset_offer().is_some())
    }

    /// Force a module onto the pill (or drop the force if it is already the one
    /// being forced) — the `debug-*` verbs' shared body. Toggling, so one verb
    /// both raises and dismisses it.
    pub(crate) fn force_module(&mut self, module: Module) {
        self.module_debug = (self.module_debug != Some(module)).then_some(module);
        // A fresh force is a fresh offer: forget that the last one was answered.
        if self.module_debug.is_some() {
            self.sunset_acted = false;
        }
        info!(
            "options: module force {}",
            self.module_debug.map_or("OFF", Module::name)
        );
        self.sync_module();
    }

    /// Which module the current-task pill should be wearing — the first in
    /// [`Module::ALL`] that wants it.
    ///
    /// The order is the precedence, and it is not arbitrary: a module that
    /// **asks something that expires** outranks one that will still be there in
    /// a minute. With a single module today it decides nothing, and it is kept
    /// because the pill is one object: the moment there are two, one of them has
    /// to lose (`OptionUXRules.md` §5).
    fn module_wanted(&self) -> Option<Module> {
        // A debug-forced module outranks the order: it exists to be looked at,
        // and a demo that loses the pill to a real offer is no demo.
        if let Some(m) = self.module_debug.filter(|m| self.module_live(*m)) {
            return Some(m);
        }
        Module::ALL.into_iter().find(|m| self.module_live(*m))
    }

    /// Whether this module has something to say right now.
    fn module_live(&self, module: Module) -> bool {
        match module {
            Module::Sunset => self.sunset_prompt_wanted(),
        }
    }

    /// Whether a module is on the pill at all (the window title has stepped
    /// aside for it). The generic gate — the controls that must stand down, the
    /// Leader that must not capture, the click that is neither answer.
    pub(crate) fn module_on_pill(&self) -> bool {
        self.module_shown.is_some()
    }

    /// Whether the pill is wearing the sunset question specifically.
    pub(crate) fn sunset_prompt_shown(&self) -> bool {
        self.module_shown == Some(Module::Sunset)
    }

    /// Bring an answered offer back to the bar: the user clicked its record in
    /// the notifications ([`crate::action_track`]) and wants to be asked again.
    /// Returns whether this offer has a surface to come back to — a caller with
    /// `false` falls back to running the action outright.
    ///
    /// The sunset prompt is the only offer with a surface of its own today, so
    /// this is a one-arm match; as ACTIONS lands, each offer's surface answers
    /// here.
    pub(crate) fn recall_offer(&mut self, id: &str) -> bool {
        if id != SUNSET_OFFER_ID {
            return false;
        }
        // A recall is a fresh asking: forget that this offer-cycle was already
        // answered, and hold the prompt up even though the Mind has long since
        // withdrawn the offer — it withdraws the moment hyprsunset runs, which
        // is exactly the state you are in when you go looking for the record.
        self.sunset_acted = false;
        self.sunset_recalled = true;
        // Straight to shown, not through `sync_module`: §2 Still Bar defers the
        // *unrequested* arrival until the pointer leaves the bar, and this
        // arrival is the user's own request (the same reason
        // `resolve_sunset_prompt` reflows at once).
        self.module_shown = Some(Module::Sunset);
        self.measure_options_text();
        self.draw_options();
        true
    }

    /// Reconcile the module the pill is DRAWING with the one that is wanted —
    /// called on every Mind republish, on the debug toggle, and when the pointer
    /// leaves the bar.
    ///
    /// The Still Bar (`OptionUXRules.md` §2): a module arriving, withdrawing or
    /// *handing over* is nobody's request, so while the pointer is on the bar
    /// strip it WAITS — the morph plays once you leave. The user's own answer
    /// goes through [`Self::resolve_sunset_prompt`] instead, which reflows at
    /// once. A spent sunset answer is forgotten once the Mind stops offering
    /// (hyprsunset runs / the sun comes back), so the next sunset asks fresh.
    pub(crate) fn sync_module(&mut self) {
        if self.sunset_acted && self.module_debug.is_none() && self.sunset_offer().is_none() {
            self.sunset_acted = false;
        }
        let mut want = self.module_wanted();
        // A module with its box open (or still animating) stays put: the box IS
        // that module expanded, so an offer withdrawing under it (picking a
        // temperature starts hyprsunset, which pulls the sunset offer; putting a
        // window on the space ends the empty room) must NOT drop the module and
        // bring the window controls back while the panel is still on screen —
        // nor hand the pill to another module underneath it. Once the box has
        // fully closed, `tick_module_box` re-syncs and the module withdraws if
        // its offer is gone.
        if self.module_box_open || self.module_box_e > 0.001 {
            want = self.module_shown.or(want);
        }
        if want == self.module_shown || self.options_ptr_on_bar() {
            return;
        }
        // The one line that records a module taking or leaving the pill — the
        // only way to tell, after the fact, whether a late arrival was the Mind
        // deciding late or the surface drawing late.
        info!(
            "options: module → {}",
            want.map_or("none", |m: Module| m.name())
        );
        self.module_shown = want;
        self.measure_options_text();
        self.draw_options();
    }

    /// The user answered (turned it on, or right-clicked "not now"): the
    /// module returns to its task NOW — this reflow is their own request.
    fn resolve_sunset_prompt(&mut self) {
        self.sunset_acted = true;
        self.module_debug = None;
        // A recalled prompt is spent by the same answer that spends a real one.
        self.sunset_recalled = false;
        // The settings box belongs to the prompt — snap it fully shut BEFORE
        // withdrawing the module, so there is never a frame with the module
        // gone but a half-open box still on screen (the window controls behind
        // a stray panel). A resolve is an abrupt answer; the box vanishes with
        // it rather than easing.
        self.module_box_open = false;
        self.module_box_e = 0.0;
        // Not simply `None`: answering hands the pill to whatever module wanted
        // it next, and the empty room may well be underneath (turning on eye
        // protection on a bare workspace is exactly that case). Re-asking rather
        // than clearing keeps the answer's reflow to ONE motion (§6) instead of
        // a withdrawal now and an arrival a republish later.
        self.module_shown = self.module_wanted();
        self.sync_options_input();
        self.measure_options_text();
        self.draw_options();
    }

    /// [turn on]: run the offer's declared action when the Mind's offer is
    /// live; the same daemon capability directly when debug-forced.
    fn sunset_turn_on(&mut self) {
        let action = self.sunset_offer().map(|a| a.action.clone());
        match action {
            Some(a) => self.run_affordance_action(&a),
            None => self.eye_protection_on(),
        }
        // Before the resolve, while the offer is still on the bar to be read.
        self.track_sunset_answer("Turned on", true);
        self.resolve_sunset_prompt();
    }

    /// File this answer as a record in the notifications (see
    /// `action_track.rs`): what was offered, what you said, when — and the
    /// offer's own action kept whole, so clicking the card runs it again.
    /// Falls back to the module's own words when the prompt is debug-forced and
    /// there is no live affordance behind it.
    fn track_sunset_answer(&mut self, answer: &str, taken: bool) {
        let (id, offered, action) = match self.sunset_offer() {
            Some(a) => (a.id.to_owned(), a.title.clone(), a.action.clone()),
            None => (
                SUNSET_OFFER_ID.to_owned(),
                SUNSET_OFFER_TITLE.to_owned(),
                options_engine::AffordanceAction::Daemon("eye_protection_on".into()),
            ),
        };
        self.track_action(&id, &offered, answer, taken, action);
    }

    /// Warm the screen. 4000K is a gentle evening warmth (hyprsunset's identity
    /// is 6500K). Goes through `screen.rs` like every other screen effect, so
    /// the choice is remembered and re-asserted rather than fired and forgotten.
    fn eye_protection_on(&mut self) {
        self.set_screen_temperature(SUNSET_TURN_ON_K);
    }

    /// The module actually on the pill as far as the DRAWING is concerned —
    /// read from the title metamorphosis, so it stays true through the whole
    /// morph in both directions (a module that is leaving is still the module
    /// being drawn). `None` for an ordinary title.
    ///
    /// This is the reason the costume can never disagree with the words: intent
    /// lives in `module_shown`, but every size, colour and clip below is keyed
    /// off the text on screen.
    fn module_drawn(&self) -> Option<Module> {
        let m = &self.options_title_meta;
        m.shown_module
            .or_else(|| m.outgoing_module.filter(|_| m.t < 0.999))
    }

    /// How present the asking module is, riding the title morph both ways:
    /// 0 = an ordinary window pill, 1 = the module fully standing. Drives
    /// the parent's size step and its opaque box surface, so the module's
    /// whole costume arrives and leaves as one movement.
    fn module_t(&self) -> f32 {
        let m = &self.options_title_meta;
        if m.shown_module.is_some() {
            m.t
        } else if m.outgoing_module.is_some() && m.t < 0.999 {
            1.0 - m.t
        } else {
            0.0
        }
    }

    /// The module's nested children — the optional `[turn on]` (sunset alone
    /// has one) and the settings gear — at the module's right end. The SINGLE
    /// source of truth for where they sit, read both by the pill layout above
    /// and by the message draw path below.
    ///
    /// The sentence must be clipped to stop at the LEFTMOST of these, not at
    /// the module's outer edge: during the arrival morph the box is still
    /// narrower than its settled width, so the box's own edge is not a tight
    /// enough bound — for most of that ease the children sit well INSIDE it, and
    /// a naive box-edge clip lets the (fixed-width, unresized) message run
    /// straight through them. Found live 2026-09-08: the sentence visibly
    /// overlapped `[turn on]` for the whole arrival morph, only snapping clear
    /// in its last few percent.
    fn module_nested_rects(&self) -> (Option<Rect>, Rect) {
        let module = self.module_drawn();
        let mt = self.module_t();
        let ph = self.options_pill_h();
        let y = PILL_MARGIN_Y;
        let cg = MODULE_CHILD_GROW * mt;
        let ih = ph + 2.0 * cg;
        let iw = self.sunset_inner_w.max(ph) + 2.0 * cg; // [turn on]
        let gw = ih; // the settings gear is a circle
        let iy = y + (MODULE_DROP_Y + MODULE_GROW_H / 2.0) * mt - cg;
        // The gear sits at the module/box's right end; [turn on] a gap to its
        // left. Anchored to the LIVE box rect, so as the box narrows into the
        // settings panel the gear rides in with it and stays the panel's
        // top-right corner.
        let br = self.module_rect();
        let gx = (br.x + br.w - PILL_PAD_X - gw).max(br.x);
        let turn_on = module.is_some_and(Module::has_turn_on).then(|| {
            let ix = (gx - SUNSET_INNER_GAP - iw).max(br.x);
            Rect::new(ix, iy, iw, ih)
        });
        (turn_on, Rect::new(gx, iy, gw, ih))
    }

    /// Where the module's sentence has to stop: a gap left of its leftmost
    /// child, or the pill's own right edge when it has none.
    fn module_msg_right(&self) -> f32 {
        let (turn_on, gear) = self.module_nested_rects();
        turn_on.unwrap_or(gear).x - MODULE_GAP
    }

    /// The banner-blister parameters (bar-edge line + fillet radius `k`) when a
    /// module is on the bar, else `None` — set into `Scene::neck`.
    /// Returned only while the blister rect is actually drawn (module
    /// shown/morphing), so the flag and the params can't disagree.
    pub(crate) fn module_neck(&self) -> Option<[f32; 4]> {
        self.module_drawn().map(|_| {
            [
                self.options_bar_h(),
                MODULE_NECK_K * self.options_scale(),
                0.0,
                0.0,
            ]
        })
    }

    /// The module's box size when fully open, in scaled px — the drawn module's
    /// own ([`Module::box_size`]), sized to the readout it holds. Falls back to
    /// the sunset panel's when no module is on the pill, which is a value
    /// nothing can read: with no module there is no box, and `module_box_e` is
    /// 0.
    pub(crate) fn module_box_size(&self) -> (f32, f32) {
        let s = self.options_scale();
        let (w, h) = self.module_drawn().unwrap_or(Module::Sunset).box_size();
        (w * s, h * s)
    }

    /// The module rect: the centred module pill, grown by its size step
    /// ([`Self::module_t`]) and then downward by `module_box_e` into the full
    /// settings panel. The one place the shape is defined: the layout draws the
    /// surface here, and [`crate::module_box`] lays each panel's content out
    /// against the same rect. With both progresses 0 it is exactly the ordinary
    /// window pill, which is what makes the arrival a morph rather than a swap.
    pub(crate) fn module_rect(&self) -> Rect {
        let sw = self.options_size.0 as f32;
        let ph = self.options_pill_h();
        let y = PILL_MARGIN_Y;
        let ww = (self.options_title_content_w() + 2.0 * PILL_PAD_X).max(ph);
        let wx = ((sw - ww) / 2.0).max(EDGE_PAD);
        let mt = self.module_t();
        let mgx = MODULE_GROW_X * mt;
        let module_w = ww + 2.0 * mgx;
        let module_h = ph + MODULE_GROW_H * mt;
        // The CENTRE stays fixed through the morph: the box narrows inward from
        // both sides and drops downward, staying centred on the bar whatever
        // the pointer did. (The gear/× rides in with the right edge — it is
        // anchored to this rect, not to the pointer that opened it.)
        let cx = wx - mgx + module_w / 2.0;
        let e = self.module_box_e;
        let (bw, bh) = self.module_box_size();
        let panel_w = bw.min(module_w);
        let box_h = bh.max(module_h);
        let w = lerp(module_w, panel_w, e);
        let h = lerp(module_h, box_h, e);
        Rect::new(cx - w / 2.0, y + MODULE_DROP_Y * mt, w, h)
    }

    /// The module's ink: the BAR's own adaptive ink (`options_text_color`).
    /// The module is now a regular OPTIONS pill (a translucent wash over the
    /// banner), so its text must read on the banner exactly like the clock and
    /// window-title do — measured against the same backdrop. It "changes" only
    /// when the whole bar's ink does (matched vs frosted), not per-module.
    pub(crate) fn module_ink(&self) -> [f32; 4] {
        self.options_text_color()
    }

    /// The module's own background alpha — the ONE fill strength shared by
    /// the parent pill's LAYER 2 and its nested [turn on]/gear children, so
    /// they read as one continuous surface rather than two independently
    /// tuned materials (Max, 2026-09-08: "the child[ren], the same color as
    /// the parent"). Boosted past the ordinary pill wash (`SUNSET_REST_ALPHA_
    /// BOOST`) at rest, easing to the open panel's own alpha as the settings
    /// box opens — each caller still applies its OWN presence multiplier
    /// (`mt` for the parent, the turn-on/gear fade for the children) on top.
    pub(crate) fn module_fill_alpha(&self) -> f32 {
        let rest_a =
            (self.options_rest_wash()[3] * MODULE_REST_ALPHA_BOOST).min(self.box_panel_alpha());
        lerp(rest_a, self.box_panel_alpha(), self.module_box_e)
    }

    /// The nested children's presence ([turn on], the gear), riding the title
    /// metamorphosis: they fade in on the sentence's own incoming ramp and back
    /// out on its outgoing one, so the module reads as ONE morph — never a pill
    /// popping onto a pill. Keyed on the morph's actual endpoints (the sentence
    /// on screen), not on intent flags, so it can't desynchronise from what is
    /// drawn.
    fn module_child_alpha(&self) -> f32 {
        let m = &self.options_title_meta;
        if m.shown_module.is_some() {
            if m.t >= 0.999 {
                1.0
            } else {
                ((m.t - TITLE_IN_START) / (1.0 - TITLE_IN_START)).clamp(0.0, 1.0)
            }
        } else if m.outgoing_module.is_some() && m.t < 0.999 {
            (1.0 - m.t / TITLE_OUT_END).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    fn options_click(&mut self) {
        // The open sunset settings box: the gear (top-right) closes it; a click
        // on a control acts; a click anywhere else closes it. Checked before
        // everything else so an open box owns the surface.
        if self.module_box_e > 0.5 {
            if self.options_hover == Some(PillId::ModuleSettings) {
                self.toggle_module_box();
                return;
            }
            if let Some((px, py)) = self.options_ptr {
                if self.module_box_click(px, py) {
                    return;
                }
                self.close_module_box();
                return;
            }
        }
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
        // Remember what was acted on and where it was drawn, so that a
        // concealment arriving right behind this click can be blamed on it and
        // leave the control standing (`OptionUXRules.md` §4).
        if let Some(id) = self.options_hover {
            if let Some(r) = self.options_pills().iter().find(|p| p.id == id) {
                self.options_acted = Some((id, r.rect, Instant::now()));
            }
        }
        match self.options_hover {
            // The gear opens PAGE 1; each reading in the readout opens its own
            // (Max, 2026-09-13: *"make each a button, everyone have a page on
            // the box"*). Pressing the button of the page already showing puts
            // the panel away — the gear's original toggle, now the rule for all
            // of them.
            Some(PillId::Settings) => self.stats_open_page(1),
            Some(PillId::SettingsStats) => {
                // The open PAGE gets the click first — it may have controls of
                // its own (page 1's floating switch). Only then the button row.
                //
                // Which reading was hit decides the page. A press on the panel
                // BELOW the row is not a button press at all — it is a press on
                // the page you are already reading, and it does nothing rather
                // than turning to whatever happens to be above it.
                if let Some((px, py)) = self.options_ptr {
                    if self.stats_panel_click(px, py) {
                        return;
                    }
                    if let Some(page) = self.stats_page_at(px, py) {
                        self.stats_open_page(page);
                    }
                }
            }
            // An empty doorway is not an action — a click on it does what
            // hovering it already did: bring the bar back (§4).
            Some(PillId::Doorway) => self.open_doorway(),
            // The cava cluster's transport. The spectrum itself is not a
            // button: clicking it does nothing, because it is information, and
            // a click that does nothing is better than a click that guesses.
            Some(PillId::CavaPlay) => {
                // Flip the face NOW — the user just told us what it should be,
                // and waiting a poll to agree is a delay with no information in
                // it. The deadline is the loan's term.
                let now_playing = self.cava_is_playing();
                self.cava_assume = Some((
                    !now_playing,
                    Instant::now() + std::time::Duration::from_millis(2500),
                ));
                self.schedule_cava_frame();
                self.cava_play_pause();
            }
            Some(PillId::CavaPrev) => self.cava_transport("previous"),
            Some(PillId::CavaNext) => self.cava_transport("next"),
            // The media glyph pill toggles the transport box.
            // While the overview owns the screen the X closes the OVERVIEW,
            // not the window under it — the bar is the overview's only
            // on-screen exit affordance (Esc/Super+R being the others).
            Some(PillId::Close) if self.overview_active => hypr::close_overview(),
            Some(PillId::Close) => {
                if let Some(addr) = self.options_active_addr.clone() {
                    hypr::close_window(&addr);
                }
            }
            // The sunset prompt's nested button: warm the screen, morph back.
            Some(PillId::SunsetTurnOn) => self.sunset_turn_on(),
            // The settings gear: expand the module into the settings box (or
            // collapse it if already open).
            Some(PillId::ModuleSettings) => self.toggle_module_box(),
            // While a module holds the pill, a click on the sentence itself does
            // nothing: for the sunset question it is neither answer (the button
            // answers yes, a right-click answers "not now"), and for the empty
            // room there is no task to cycle focus to. The sentence is text, not
            // a button.
            Some(PillId::Window) if self.module_on_pill() => {}
            // The current-task pill cycles focus through this workspace's
            // windows, most-used first (see `crate::focus_cycle`).
            Some(PillId::Window) => self.cycle_focus(true),
            // The window-mode controls. One mode at a time: entering one leaves
            // whatever was on, and pressing the mode you are already in returns
            // the window to the layout (see [`hypr::set_window_mode`]).
            // The state pill is the control for the mode it SHOWS — the one
            // whose glyph is under the finger, never the live mode.
            //
            // The difference is the whole toggle. Press it once and the window
            // leaves that mode, but the pill stays put (§2, the row is frozen
            // while the hand is on the bar), so the obvious second press means
            // "put it back". Acting on the live mode instead asked for the mode
            // the window had *just* reached — tiled — which is where it already
            // was, so nothing happened and the button read as stuck until you
            // walked away and came back (Max, 2026-09-13).
            Some(PillId::WindowState) => self.set_window_mode(self.options_mode_shown),
            Some(PillId::Pseudo) => self.set_window_mode(hypr::WindowMode::Pseudo),
            Some(PillId::Float) => self.set_window_mode(hypr::WindowMode::Floating),
            Some(PillId::Fullscreen) => self.set_window_mode(hypr::WindowMode::Fullscreen),
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
            // The stage's mode switch: one task alone, or the whole desk.
            Some(PillId::StageMode) => self.toggle_stage_mode(),
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
    ///
    /// The `Daemon(tag)` vocabulary is stringly-typed across two crates: keep
    /// the match arms in `run_affordance_action` and [`daemon_tag_known`] in
    /// lockstep, or a tag added on the engine side without a dispatch arm
    /// ships as a pill that logs a warn and does nothing.
    ///
    /// The cross-crate test that walked the ENGINE's actual emissions against
    /// `daemon_tag_known` was removed with the providers (2026-09-12) — it
    /// asserted specific tags were emitted, and no provider emits anything
    /// now. **Restore it alongside the first curated OPTION that carries a
    /// `Daemon(tag)`**, or this seam goes back to being unguarded.
    pub(crate) fn run_affordance_action(&mut self, action: &options_engine::AffordanceAction) {
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
                // The sunset offer: warm the screen (hyprsunset).
                "eye_protection_on" => self.eye_protection_on(),
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
                // Undo in the focused creative app — Ctrl+Z is the one chord
                // that is universal across the image/video editors.
                "undo" => crate::hypr::send_shortcut_active("CTRL", "z"),
                // Empty the FreeDesktop trash (the disk-almost-full remedy),
                // then refilter so an open Recycle Bin view empties too.
                "empty_trash" => {
                    match crate::trash::Trash::home().empty() {
                        Ok(()) => info!("options: emptied the trash"),
                        Err(e) => warn!("options: emptying the trash failed: {e}"),
                    }
                    self.refilter();
                }
                // "define:<word>" — open the clipboard box's dictionary panel
                // pre-filled with a copied word (the selection module's
                // "Define word" pill).
                t if t.starts_with("define:") => self.open_define(&t["define:".len()..]),
                // "pkgsearch:<name>" — open the launcher's Install search
                // pre-filled with a package name (a command-not-found remedy).
                t if t.starts_with("pkgsearch:") => self.pkg_search_for(&t["pkgsearch:".len()..]),
                other => {
                    debug_assert!(
                        !daemon_tag_known(other),
                        "daemon_tag_known says '{other}' is handled but no match arm took it"
                    );
                    warn!("options: unknown daemon action '{other}'");
                }
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
        // The sunset prompt's "not now": a right-click anywhere on the asking
        // module resolves it for this offer (the Mind re-offers next sunset).
        if self.sunset_prompt_shown()
            && matches!(
                self.options_hover,
                Some(PillId::Window | PillId::SunsetTurnOn | PillId::ModuleSettings)
            )
        {
            self.track_sunset_answer("Not now", false);
            self.resolve_sunset_prompt();
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
            // The gear and every reading in its readout open a page now, so
            // they may claim to be buttons — which they could not while they
            // did nothing (a pointer cursor over a dead pill is the surface
            // saying something untrue). The panel BELOW the row is not a button
            // though: it is the page you are reading, and it says so by leaving
            // the cursor alone.
            Some(PillId::Settings) => Shape::Pointer,
            Some(PillId::SettingsStats) => match self.options_ptr {
                Some((px, py)) if self.stats_page_at(px, py).is_some() => Shape::Pointer,
                _ => Shape::Default,
            },
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

/// The dock's twin of the bar's adaptive-ink machinery above (see
/// `options_regime`/`options_bar_is_bright`/`options_box_surface`) — same
/// [`Backdrop`], read from `dock_bar_matched`/`dock_pill_color` instead of
/// the bar's fields. Unlike the bar, the dock has no separate pill/box
/// split: docked or open it is one continuous card (`content::scene`'s
/// single `card_rect`), so there is only one fill to compute, not two.
impl crate::App {
    /// The dock's live colour regime — matched window, else sampled frost.
    fn dock_regime(&self) -> Backdrop {
        Backdrop {
            matched: self.dock_bar_matched,
            frost: self.dock_pill_color,
        }
    }

    /// Hover wash for a highlighted dock/grid row — the stronger sibling of
    /// the dock's own resting wash, same asymmetry as `options_hover_wash`.
    pub(crate) fn dock_hover_wash(&self) -> [f32; 4] {
        self.dock_regime().hover_wash()
    }

    /// The three border-gradient stops, top to bottom. THE definition of
    /// "the border colour" for the whole shell: the compositor push below
    /// wears them on real windows, and every border the shell draws itself
    /// for a *miniature* of a window (the STAGE deck's tiles) takes its
    /// colour from the same three, so a tile's frame can never drift from
    /// the frame around the window it stands for.
    ///
    /// **The stage borrows the desktop's.** STAGE mode dims the whole
    /// backdrop on purpose (`dim_around = 0.8`), and all three samples read
    /// that backdrop — so sampling live there collapses every stop toward
    /// black, and the lightness step turns the collapse into grey. That is
    /// precisely when this colour is most on show: the deck tiles' frames
    /// and the staged window's border are made of it (Max, 2026-09-11: the
    /// deck tile borders "go kind of gray after the images of the tiles
    /// load" — the dim fading in, not the thumbnails). A deliberate dim
    /// must not redefine the shell's colour, so while the stage is up we
    /// keep the last colour the desktop had.
    pub(crate) fn border_stops(&self) -> [[f32; 4]; 3] {
        if self.stage.is_on() {
            if let Some(desktop) = self.border_desktop {
                return desktop;
            }
        }
        /// Border-strength lightness step (the plate/zebra recipe, turned
        /// up): each stop keeps its region's HUE but moves a clear step
        /// away in lightness — lifted over a dark sample, dimmed over a
        /// bright one — so the frame reads on any wallpaper instead of
        /// dissolving into it (Max, 2026-09-11: "there is no contrast").
        const BORDER_LIFT_L: f32 = 0.30;
        const BORDER_DIM_L: f32 = 0.24;
        let bg = self.config.theme.background_rgba();
        let ink = self.config.theme.text_rgba();
        let stop = |b: Backdrop| {
            let fill = b.surface(bg, ink, false).0;
            let srgb = [
                linear_to_srgb(fill[0]).clamp(0.0, 1.0),
                linear_to_srgb(fill[1]).clamp(0.0, 1.0),
                linear_to_srgb(fill[2]).clamp(0.0, 1.0),
            ];
            let (h, s, l) = rgb_to_hsl(srgb[0], srgb[1], srgb[2]);
            let new_l = if luminance(fill) <= 0.179 {
                (l + BORDER_LIFT_L).min(1.0)
            } else {
                (l - BORDER_DIM_L).max(0.0)
            };
            let (r, g, b) = hsl_to_rgb(h, s, new_l);
            [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0]
        };
        [
            stop(self.dock_regime()),
            stop(self.options_regime()),
            stop(self.clip_regime()),
        ]
    }

    /// One colour standing for [`Self::border_stops`], for a frame too
    /// small to carry a gradient (a deck tile is a few hundred px wide —
    /// three stops across it would read as one muddy average anyway, so
    /// average them honestly instead). Opaque, like the stops.
    pub(crate) fn border_tint(&self) -> [f32; 4] {
        let stops = self.border_stops();
        let mean = |i: usize| stops.iter().map(|s| s[i]).sum::<f32>() / stops.len() as f32;
        [mean(0), mean(1), mean(2), 1.0]
    }

    /// The window borders follow the shell's three screen samples as a
    /// vertical gradient, each colour placed on the OPPOSITE side of the
    /// frame from where it was sampled (Max, 2026-09-11: "the sample we
    /// take at the top, put it down") so the border always separates from
    /// the wallpaper around it: the dock's colour (sampled at the screen
    /// bottom) tops the frame; the notif-side and clipboard-side colours
    /// (sampled along the top edge) sink to the bottom. Angle 90 puts the
    /// FIRST stop at the top (calibrated live, 2026-09-11).
    ///
    /// Each stop is its regime's `surface()` fill — matched window colour
    /// or wallpaper frost, mixed exactly like the surfaces themselves.
    /// Pushed to Hyprland via runtime `hl.config` only when a stop actually
    /// moves (small-delta throttle keeps sampling noise off the socket);
    /// the compositor's own border animation (`leaf = "border"` in
    /// hyprland.lua) does the easing, so pushing targets rather than eased
    /// values is exactly right. Best effort like all hypr IPC — without
    /// Hyprland nothing happens.
    ///
    /// Driven by the COLOUR pipeline (`screencopy`: a sample landing, or a
    /// regime re-evaluation), never by the frame loop. It first hung off
    /// `dock_surface_eased`, which runs once per drawn frame: a transition
    /// then fired this *blocking* compositor round-trip at up to the
    /// refresh rate, starving the very draws the colour switch was riding
    /// — the switch appeared to stick (Max, 2026-09-11). A hidden dock
    /// draws nothing at all, so the border also stopped following the
    /// screen entirely whenever the dock was away.
    pub(crate) fn push_window_border(&mut self) {
        const EPS: f32 = 0.006; // ~1.5/255 per linear channel
        let stops = self.border_stops();
        // Remember what the desktop looks like, so the stage has something
        // to borrow rather than sampling its own dim (see `border_stops`).
        if !self.stage.is_on() {
            self.border_desktop = Some(stops);
        }
        if self.border_pushed.is_some_and(|last| {
            last.iter()
                .flatten()
                .zip(stops.iter().flatten())
                .all(|(a, b)| (a - b).abs() < EPS)
        }) {
            return;
        }
        self.border_pushed = Some(stops);
        let hex = |c: [f32; 4]| {
            let byte = |v: f32| (linear_to_srgb(v).clamp(0.0, 1.0) * 255.0).round() as u8;
            format!(
                "rgba({:02x}{:02x}{:02x}ff)",
                byte(c[0]),
                byte(c[1]),
                byte(c[2])
            )
        };
        crate::hypr::eval(&format!(
            "hl.config({{ general = {{ col = {{ active_border = {{ colors = {{ '{}', '{}', '{}' }}, angle = 90 }} }} }} }})",
            hex(stops[0]),
            hex(stops[1]),
            hex(stops[2]),
        ));
    }

    /// The dock card's fill, and the ink that reads on it — one call, same
    /// "the surface is the pill grown" formula as `options_box_surface`
    /// (backdrop plus the resting wash, ink measured against that same
    /// result so the two always agree).
    ///
    /// The returned fill's alpha is the THEME's own translucency
    /// (`background`'s alpha channel), not forced opaque like
    /// `options_box_surface`: the dock's Hyprland layer rule blurs through
    /// exactly that transparency (`ignore_alpha = 0.5` in `hyprland.lua`) —
    /// the AGUA glass look depends on it, so unlike an OPTIONS box this fill
    /// must stay translucent. Ink is unaffected either way — `luminance`
    /// never reads the alpha channel.
    pub(crate) fn dock_surface(&self) -> ([f32; 4], [f32; 4]) {
        let bg = self.config.theme.background_rgba();
        let ink = self.config.theme.text_rgba();
        self.dock_regime().surface(bg, ink, false)
    }

    /// [`Self::dock_surface`], temporally smoothed: the sampled colour only
    /// sets *targets*; the drawn fill, ink, and hover wash each ease toward
    /// theirs every frame instead of repainting in one hard set — a new
    /// sample (window match acquired or dropped, wallpaper region change)
    /// fades in over ~¼ s rather than blinking. The ink's *decision* is
    /// still measured against the eased fill (it flips the moment the fill
    /// actually crosses the readability threshold, never ahead of what's on
    /// screen), but the flip itself is a quick crossfade too — slightly
    /// faster than the fill, so text is only ever briefly mid-grey.
    ///
    /// While the dock is fully hidden everything snaps: colour changes that
    /// happen off-screen (a workspace switch behind a hidden dock) must not
    /// play as a fade during the reveal slide — the dock arrives already
    /// wearing the right colour.
    ///
    /// The caller keeps frames coming while [`DockPaint::moving`] is set.
    pub(crate) fn dock_surface_eased(&mut self, dt: f32) -> DockPaint {
        /// Fill approach rate (s⁻¹): τ ≈ 45 ms, settled in ~140 ms — just
        /// enough blend to kill the one-frame blink, still reads instant.
        const DOCK_FILL_RATE: f32 = 22.0;
        /// Ink/wash approach rate (s⁻¹): faster still, so text spends the
        /// least time between its two legible extremes.
        const DOCK_INK_RATE: f32 = 30.0;
        /// Per-channel ease of an RGBA value toward `target`; `None` (and
        /// the hidden snap) seed instantly.
        fn ease_rgba(
            cur: &mut Option<[f32; 4]>,
            target: [f32; 4],
            dt: f32,
            rate: f32,
            snap: bool,
        ) -> ([f32; 4], bool) {
            if snap {
                *cur = Some(target);
                return (target, false);
            }
            let cur = cur.get_or_insert(target);
            let mut moving = false;
            for (c, t) in cur.iter_mut().zip(target) {
                let (v, m) = crate::animation::ease_toward(*c, t, dt, rate, 0.002);
                *c = v;
                moving |= m;
            }
            (*cur, moving)
        }
        let hidden = self.ui.reveal() <= 0.001;
        let (fill_target, fallback_ink) = self.dock_surface();
        let (fill, fill_moving) = ease_rgba(
            &mut self.dock_fill_anim,
            fill_target,
            dt,
            DOCK_FILL_RATE,
            hidden,
        );
        // Ink decision from the eased fill while a sample drives it — the
        // same rule the bar uses (the dock used to need its own because it
        // wanted pure white where the bar wanted warm off-white; now that
        // OPTIONS' ink is real black and white there is one rule); the
        // theme's own ink for the (brief) sampleless fallback.
        let ink_target = if self.dock_regime().get().is_some() {
            ink_on(fill)
        } else {
            fallback_ink
        };
        let (ink, ink_moving) = ease_rgba(
            &mut self.dock_ink_anim,
            ink_target,
            dt,
            DOCK_INK_RATE,
            hidden,
        );
        let wash_target = self.dock_hover_wash();
        let (wash, wash_moving) = ease_rgba(
            &mut self.dock_wash_anim,
            wash_target,
            dt,
            DOCK_INK_RATE,
            hidden,
        );
        // Icon squircle plates: the fill itself, one lightness step away at
        // its OWN hue — the `zebra_stripe` recipe. A plain white/black
        // frost was tried first and read as one fixed colour no matter what
        // the surface did (Max, 2026-09-10); shifting HSL lightness of the
        // eased fill makes a plate that is visibly "THIS surface's colour,
        // one shade lighter/darker" — a green-matched dock gets green
        // plates. Lift on dark fills, dim on bright, computed in sRGB where
        // L is perceptually meaningful.
        //
        // One strength for BOTH states: a docked/open blend (subtle frost
        // at rest, strong chip open) was tried and Max explicitly chose
        // the strong open look everywhere — "make the dock look like
        // that" (2026-09-10).
        /// Plate lightness lift over a dark fill / dim under a bright one.
        const PLATE_LIFT_L: f32 = 0.20;
        const PLATE_DIM_L: f32 = 0.16;
        /// Plate opacity — strong enough that the hue clearly reads.
        /// Was 0.85; stepped down and settled at 0.40 (Max, 2026-09-10 —
        /// 0.30 read too faint once the rim landed). Same strength for
        /// ALL plates, dock and grid alike.
        const PLATE_ALPHA: f32 = 0.50;
        let plate_target = {
            let (lift, dim, alpha) = (PLATE_LIFT_L, PLATE_DIM_L, PLATE_ALPHA);
            let srgb = [
                linear_to_srgb(fill[0]).clamp(0.0, 1.0),
                linear_to_srgb(fill[1]).clamp(0.0, 1.0),
                linear_to_srgb(fill[2]).clamp(0.0, 1.0),
            ];
            let (h, s, l) = rgb_to_hsl(srgb[0], srgb[1], srgb[2]);
            let new_l = if luminance(fill) <= 0.179 {
                (l + lift).min(1.0)
            } else {
                (l - dim).max(0.0)
            };
            let (r, g, b) = hsl_to_rgb(h, s, new_l);
            [
                srgb_to_linear(r),
                srgb_to_linear(g),
                srgb_to_linear(b),
                alpha,
            ]
        };
        let (plate, plate_moving) = ease_rgba(
            &mut self.dock_plate_anim,
            plate_target,
            dt,
            DOCK_INK_RATE,
            hidden,
        );
        DockPaint {
            fill,
            ink,
            wash,
            plate,
            moving: fill_moving || ink_moving || wash_moving || plate_moving,
        }
    }
}

/// The dock's eased on-screen colours for one frame — everything
/// [`crate::App::dock_surface_eased`] animates, in one bundle.
pub(crate) struct DockPaint {
    /// Card fill (the adaptive glass colour).
    pub fill: [f32; 4],
    /// Ink that reads on that fill (labels, glyphs, dots).
    pub ink: [f32; 4],
    /// Hover/selection wash.
    pub wash: [f32; 4],
    /// Icon squircle-plate colour (shader-drawn, see `IconInst::plate`).
    pub plate: [f32; 4],
    /// True while any of the four is still easing — keep frames coming.
    pub moving: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use options_engine::AffordanceAction;

    fn pill_at(id: PillId, x: f32, w: f32) -> Pill {
        Pill {
            id,
            rect: Rect::new(x, 2.5, w, 25.0),
            text: String::new(),
            family: None,
            glyph_color: None,
        }
    }

    /// An open box takes the bar it stands on: every pill under it leaves, its
    /// own parts stay, and the pills clear of it are untouched — including
    /// their POSITIONS, since a re-flow under the pointer is exactly what
    /// `OptionUXRules.md` §2 forbids.
    #[test]
    fn an_open_box_clears_the_pills_it_covers() {
        let mut pills = vec![
            pill_at(PillId::Settings, 6.0, 28.0),   // left of the box
            pill_at(PillId::Clipboard, 40.0, 28.0), // the box's own pill
            pill_at(PillId::Cava, 120.0, 60.0),     // fully under it
            pill_at(PillId::CavaPlay, 150.0, 12.0), // its glyph, ditto
            pill_at(PillId::Clock, 500.0, 60.0),    // clear to the right
        ];
        let before_clock = pills[4].rect.x;
        clear_under(&mut pills, Rect::new(40.0, 2.5, 380.0, 500.0), |id| {
            matches!(id, PillId::Clipboard | PillId::Settings)
        });
        let left: Vec<PillId> = pills.iter().map(|p| p.id).collect();
        assert_eq!(
            left,
            vec![PillId::Settings, PillId::Clipboard, PillId::Clock],
            "the covered pills should have left the bar, and only those"
        );
        assert_eq!(pills[2].rect.x, before_clock, "the survivors must not move");
    }

    /// Touching is not covering: a pill that only abuts the box's edge stays.
    #[test]
    fn a_pill_beside_an_open_box_survives() {
        let mut pills = vec![pill_at(PillId::Cava, 420.0, 60.0)];
        clear_under(&mut pills, Rect::new(40.0, 2.5, 380.0, 500.0), |_| false);
        assert_eq!(pills.len(), 1, "x = box right edge is not under the box");
    }

    /// The clock is furniture: it covers, it is never covered. The bell's peek
    /// used to take it away the moment the date grew into the preview's span —
    /// hover the bell, slide right, and the clock disappeared.
    #[test]
    fn nothing_takes_the_clock_away() {
        let mut pills = vec![
            pill_at(PillId::Notif, 1500.0, 380.0),
            pill_at(PillId::Clock, 1744.0, 250.0),
        ];
        clear_under(&mut pills, Rect::new(1500.0, 2.5, 380.0, 28.0), |id| {
            matches!(id, PillId::Notif | PillId::NotifMute)
        });
        assert!(
            pills.iter().any(|p| p.id == PillId::Clock),
            "the clock must survive whatever stands on it"
        );
    }

    /// Everything a module owes the pill it takes over, checked for every one
    /// there is: a sentence to wear (an empty one leaves a blank pill standing
    /// on the bar), a panel header that names it as a place rather than
    /// repeating its sentence, and a box big enough to be a panel.
    ///
    /// `has_children` is what decides whether the sentence is left-anchored and
    /// clipped at its leftmost child or centred like an ordinary title, so it
    /// must follow from the children actually being laid out, never be set by
    /// hand. The gear is universal; only a module that ASKS something carries an
    /// action pill to answer with.
    #[test]
    fn every_module_can_wear_the_pill() {
        assert!(
            Module::Sunset.has_turn_on(),
            "it asks, so it can be answered"
        );
        for m in Module::ALL {
            assert!(!m.msg().is_empty(), "{m:?} would stand there blank");
            assert!(m.has_gear(), "{m:?} has a settings box to reach");
            assert_eq!(
                m.has_children(),
                m.has_gear() || m.has_turn_on(),
                "{m:?}'s anchoring must follow from what it actually nests"
            );
            assert!(!m.panel_title().is_empty(), "{m:?} panel has no header");
            assert_ne!(m.panel_title(), m.msg());
            let (w, h) = m.box_size();
            assert!(w > 0.0 && h > 0.0, "{m:?} has no box");
        }
    }

    /// The module offers have their own surface (this pill), so they must never
    /// also appear as a generic cluster glyph — the rule battery and deploy
    /// already follow.
    #[test]
    fn a_module_offer_is_never_also_a_cluster_pill() {
        let offer = |id: &'static str| options_engine::Affordance {
            id,
            kind: options_engine::AffordanceKind::Control,
            title: "t".into(),
            detail: String::new(),
            relevance: 0.9,
            reason: "test",
            source: options_engine::Layer::Compositor,
            // Actionable, i.e. it WOULD be surfaced if it were not a module.
            action: AffordanceAction::Daemon("noop".into()),
            immediate: false,
            shows_in: options_engine::ShowsIn::Context,
        };
        assert!(!is_surfaced_affordance(&offer(SUNSET_OFFER_ID)));
        assert!(is_surfaced_affordance(&offer("something.else")));
    }

    #[test]
    fn every_id_gets_the_generic_glyph_while_the_table_is_empty() {
        // The per-id arms went with the uncurated offers (2026-09-12). What
        // must survive is the fallback: an id with no arm gets a glyph, never
        // a crash — so a curated OPTION renders from the day it is added,
        // before anyone has chosen its icon.
        assert_eq!(glyph_for_option("something.new", ""), GLYPH_OPTION);
        assert_eq!(glyph_for_option("", ""), GLYPH_OPTION);
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

    #[test]
    fn an_overview_hover_carries_the_window_and_the_words() {
        let (addr, title) = split_overview_hover("0x55f3abc Firefox — the web");
        assert_eq!(addr.as_deref(), Some("0x55f3abc"));
        assert_eq!(title.as_deref(), Some("Firefox — the web"));
        // Leaving a thumbnail ends both.
        assert_eq!(split_overview_hover(""), (None, None));
        // A window with no title yet is still a window.
        assert_eq!(
            split_overview_hover("0x55f3abc"),
            (Some("0x55f3abc".to_owned()), None)
        );
        // An older plugin sends the title alone — it must land as a title, not
        // as an address the stage would then try to focus.
        let (addr, title) = split_overview_hover("Firefox — the web");
        assert_eq!(addr, None);
        assert_eq!(title.as_deref(), Some("Firefox — the web"));
    }

    #[test]
    fn tooltip_estimate_is_generous_and_class_aware() {
        // Caps-heavy strings must estimate wider than same-length lowercase
        // (the live bug: a flat 0.62/char average under-sized "PROBE-…" and
        // the label wrapped out of existence).
        let caps = est_text_w("PROBE-TRADUCIDO", 17.0);
        let lower = est_text_w("probe-traducido", 17.0);
        assert!(caps > lower);
        // The old flat estimate for this string was 15 × 17 × 0.62 ≈ 158 —
        // provably too small; the class-aware one clears it with margin.
        assert!(caps > 170.0, "caps estimate {caps} still too tight");
        // Monotonic in content; empty is zero.
        assert_eq!(est_text_w("", 17.0), 0.0);
        assert!(est_text_w("Open copied files", 17.0) > est_text_w("Open", 17.0));
        // Unknown scripts assume a full em — never narrower than Latin.
        assert!(est_text_w("日本語", 17.0) >= 3.0 * 17.0);
    }

    // --- The Leader (OptionUXRules.md §1) -----------------------------------

    /// The resting x of `[X]`, straight from the layout in
    /// [`App::options_pills_resting`]: the name pill is centred alone and the
    /// close rests a gap to its right, so the close rides on HALF the title
    /// width — the whole reason this rule exists.
    fn resting_close_x(bar_w: f32, title_w: f32, ph: f32) -> f32 {
        let ww = (title_w + 2.0 * PILL_PAD_X).max(ph);
        let wx = ((bar_w - ww) / 2.0).max(EDGE_PAD);
        wx + ww + GROUP_GAP
    }

    #[test]
    fn leader_holds_a_fixed_width_button_exactly_still() {
        // The live case: click [X], the Firefox tile closes, the compositor
        // focuses a `foot` with a much shorter name.
        let (bar_w, ph) = (1920.0, 27.0);
        let before = resting_close_x(bar_w, 380.0, ph);
        let after = resting_close_x(bar_w, 60.0, ph);
        // Without the rule the button runs away by half the title delta.
        assert!(
            (before - after - 160.0).abs() < 0.01,
            "close moves {} px unaided",
            before - after
        );
        // Anchored where the click found it; the layout then re-flows under it.
        let anchor = before + ph / 2.0;
        let drawn = after + lead_shift(Rect::new(after, 0.0, ph, ph), anchor);
        assert!((drawn - before).abs() < 1e-4, "leader moved to {drawn}");
    }

    #[test]
    fn travelling_to_a_neighbour_does_not_push_it_away() {
        // The live bug that reshaped this rule (2026-09-04): with the leader
        // pinned under the CURSOR, setting off from [current task] toward [X]
        // dragged the whole cluster along — [X] retreated at exactly the speed
        // it was chased and could never be reached. The anchor is a place on
        // the bar, so a pointer that is merely travelling displaces nothing.
        let (bar_w, ph) = (1920.0, 27.0);
        let title_w = 380.0;
        let ww = (title_w + 2.0 * PILL_PAD_X).max(ph);
        let wx = ((bar_w - ww) / 2.0).max(EDGE_PAD);
        let name = Rect::new(wx, 0.0, ww, ph);
        // Click the middle of the name pill, then walk right toward [X].
        let anchor = wx + ww / 2.0;
        let close = resting_close_x(bar_w, title_w, ph);
        let mut reached = false;
        for step in 0..400 {
            let ptr = anchor + step as f32;
            // The layout has not changed, so nothing may move...
            let shift = lead_shift(name, anchor);
            assert_eq!(shift, 0.0, "the cluster moved while the pointer travelled");
            // ...which means the walk actually arrives on [X].
            if ptr >= close + shift && ptr <= close + shift + ph {
                reached = true;
                break;
            }
        }
        assert!(reached, "the pointer never caught up with [X]");
    }

    #[test]
    fn the_anchor_is_the_leaders_centre() {
        // A leader that resizes holds its PLACE, not one of its edges: the
        // width change is spent evenly on both sides instead of lunging one
        // way. (For the fixed-width buttons that do the re-flowing work,
        // centre and edges are the same promise.)
        let anchor = 500.0;
        for w in [27.0, 120.0, 400.0] {
            let nat = Rect::new(300.0, 0.0, w, 27.0);
            let drawn = nat.x + lead_shift(nat, anchor);
            assert!(
                (drawn + w / 2.0 - anchor).abs() < 1e-4,
                "width {w} moved the leader off its anchor"
            );
        }
    }

    #[test]
    fn a_mind_control_holds_its_place_when_the_row_re_ranks() {
        // The Mind's row is left-anchored: when an offer above withdraws,
        // every control below it slides left by a slot. The leader is held by
        // its ACTION, so the pill that was clicked keeps its place on the bar
        // and the re-rank plays out around it.
        let ph = 27.0;
        let slot = |i: f32| Rect::new(200.0 + i * (ph + CTRL_GAP), 0.0, ph, ph);
        // It moves a whole slot unaided — the misfire being prevented.
        assert!((slot(2.0).x - slot(1.0).x - (ph + CTRL_GAP)).abs() < 1e-4);
        // Clicked in the third slot; the first offer then withdraws and the
        // action is re-ranked into the second.
        let anchor = slot(2.0).x + ph / 2.0;
        let drawn = slot(1.0).x + lead_shift(slot(1.0), anchor);
        assert!(
            (drawn - slot(2.0).x).abs() < 1e-4,
            "control moved to {drawn}"
        );
    }

    #[test]
    fn leader_shift_is_zero_when_the_layout_does_not_move() {
        // At rest, nothing changed: a leader whose pill has not moved displaces
        // its group by exactly 0, so the drawn layout IS the resting layout.
        for (x, w) in [(860.0, 27.0), (400.0, 300.0), (EDGE_PAD, 27.0)] {
            assert_eq!(
                lead_shift(Rect::new(x, 0.0, w, 27.0), x + w / 2.0),
                0.0,
                "a {w}px pill that did not move was nudged"
            );
        }
    }

    #[test]
    fn the_leader_yields_to_the_edges() {
        let span = (700.0, 900.0);
        let edge = (EDGE_PAD, 1920.0 - EDGE_PAD);
        // Free on both sides: the shift passes through untouched.
        assert_eq!(
            clamp_shift(span, None, None, edge, OPTION_GAP, -120.0),
            -120.0
        );
        // A neighbour at 640 on the left: the group may only come back to
        // 640 + OPTION_GAP, so a bigger leftward shift is cut short.
        let s = clamp_shift(span, Some(640.0), None, edge, OPTION_GAP, -120.0);
        assert!((s - (640.0 + OPTION_GAP - 700.0)).abs() < 1e-4);
        assert!(s > -120.0, "clamp must reduce the shift, not grow it");
        // A neighbour at 950 on the right bounds the other direction.
        let r = clamp_shift(span, None, Some(950.0), edge, OPTION_GAP, 200.0);
        assert!((r - (950.0 - OPTION_GAP - 900.0)).abs() < 1e-4);
        // Squeezed from both sides (a group already too wide for its slot):
        // deterministic, and never NaN.
        let both = clamp_shift(span, Some(690.0), Some(710.0), edge, OPTION_GAP, 50.0);
        assert!(both.is_finite());
    }

    #[test]
    fn only_the_reflowing_groups_can_be_led() {
        // The window cluster moves as one unit — that is what "its OPTION
        // lays out from the leader" means.
        for id in [
            PillId::Window,
            PillId::Close,
            PillId::Pseudo,
            PillId::Fullscreen,
        ] {
            assert_eq!(group_of(id), PillGroup::Window);
        }
        assert_eq!(group_of(PillId::Option(0)), PillGroup::Mind);
        assert_eq!(group_of(PillId::NotifMute), PillGroup::Notif);
        assert_eq!(group_of(PillId::ClipCopyLink), PillGroup::Clipboard);
        // Leadable = the two that re-flow, on distinct slots; the edge-pinned
        // OPTIONS own their own morphs and are left alone.
        assert_eq!(group_slot(PillGroup::Window), Some(0));
        assert_eq!(group_slot(PillGroup::Mind), Some(1));
        for g in [PillGroup::Clock, PillGroup::Notif, PillGroup::Clipboard] {
            assert_eq!(group_slot(g), None);
        }
        assert!(group_slot(PillGroup::Window).unwrap() < LEAD_N);
        assert!(group_slot(PillGroup::Mind).unwrap() < LEAD_N);
    }

    #[test]
    fn held_still_the_repeat_click_lands_on_the_same_control() {
        // The misfire this rule exists to stop: without it, a shrinking title
        // walks [pseudo] into the space [X] vacated, so a second click
        // pseudotiles instead of closing. Four closes in a row, no re-aim.
        let (bar_w, ph) = (1920.0, 27.0);
        let titles = [380.0, 240.0, 60.0, 150.0, 20.0];
        // Anchored by the first click; the pointer then never moves again.
        let first = resting_close_x(bar_w, titles[0], ph);
        let anchor = first + ph / 2.0;
        let ptr = first + 13.0;
        for w in &titles[1..] {
            let nat = Rect::new(resting_close_x(bar_w, *w, ph), 0.0, ph, ph);
            let drawn = nat.x + lead_shift(nat, anchor);
            assert!((drawn - first).abs() < 1e-4, "[X] drifted to {drawn}");
            // The pointer is still inside [X], never past it into [pseudo]
            // (which rests GROUP_GAP beyond the close's right edge).
            assert!(ptr >= drawn && ptr <= drawn + ph, "pointer left [X]");
            assert!(ptr < drawn + ph + GROUP_GAP, "pointer reached [pseudo]");
        }
    }

    #[test]
    fn the_date_collapses_when_you_leave_the_pill() {
        // Every element on this bar retracts when you step off IT; the clock
        // used to be the exception, waiting for the pointer to leave the whole
        // banner (Max, 2026-09-13: *"i want the clock to colapse when i leave
        // the pill too"*). That exception existed for a real reason — the bell
        // was pinned to the clock's live edge, so collapsing dragged it out
        // from under a pointer aiming at it — and it went away when the bell
        // stopped moving (`options_clock_rest_left`).
        assert!(
            clock_may_collapse(true, false, false),
            "off the clock and still showing the date: this must collapse"
        );
        // Still on the pill: nothing to do, the date is being read.
        assert!(!clock_may_collapse(true, false, true));
        // Nothing to collapse, or a collapse already armed: no second timer.
        assert!(!clock_may_collapse(false, false, false));
        assert!(!clock_may_collapse(true, true, false));
    }

    // --- Sticky OPTIONS (OptionUXRules.md §4) -------------------------------

    #[test]
    fn the_doorway_stands_where_its_successor_will() {
        // §4: "it becomes [pseudo]" has to be literal. The doorway takes the
        // exact slot the next control along occupies in the real layout, so
        // when the bar returns that pill arrives *in place* — a metamorphosis,
        // not a pill swapped for a different pill somewhere else.
        let (bar_w, ph) = (1920.0, 27.0);
        let close = resting_close_x(bar_w, 380.0, ph);
        // Resting: [X] [pseudo] [fullscreen], pseudo a GROUP_GAP past the
        // close, fullscreen a CTRL_GAP past pseudo.
        let pseudo = close + ph + GROUP_GAP;
        let full = pseudo + ph + CTRL_GAP;
        // The doorway sits one slot back from the sticky control.
        let door = full - (ph + CTRL_GAP);
        assert!(
            (door - pseudo).abs() < 1e-4,
            "doorway at {door} does not stand where [pseudo] does ({pseudo})"
        );
    }

    #[test]
    fn the_way_back_is_never_wider_than_the_thing_it_replaces() {
        // §4's trade is one moment of chrome against a journey every time. Two
        // pills is the price; anything more and the rule is buying the user's
        // fullscreen back at too high a rate.
        let ph = 27.0;
        let sticky = Rect::new(900.0, 2.5, ph, ph);
        let door = Rect::new(sticky.x - (ph + CTRL_GAP), sticky.y, ph, ph);
        let span = (sticky.x + sticky.w) - door.x;
        assert!(
            span <= 2.0 * ph + CTRL_GAP + 1e-4,
            "the sticky pair spans {span}px — more than the two pills it is"
        );
        // And it stays inside the bar's own strip: it is a survivor of the bar,
        // not a new surface somewhere else on the screen.
        assert!(door.y >= 0.0 && door.y + door.h <= ph + 2.0 * PILL_MARGIN_Y + 1e-4);
    }

    #[test]
    fn pill_scale_full_size_on_tall_screens_and_before_outputs() {
        // At or above the full-size threshold: exactly 1.0 (large screens
        // provably unchanged).
        assert_eq!(pill_scale_for(Some(900.0)), 1.0);
        assert_eq!(pill_scale_for(Some(1440.0)), 1.0);
        // Output size not yet known (startup, pre-enumeration): full size —
        // matching the exclusive zone set at surface creation.
        assert_eq!(pill_scale_for(None), 1.0);
    }

    #[test]
    fn pill_scale_shrinks_small_screens_to_a_floor() {
        // The 1280x800 VM screen: 800/900.
        let s = pill_scale_for(Some(800.0));
        assert!((s - 800.0 / 900.0).abs() < 1e-6);
        // The 1366x768 Acer: 768/900, still above the floor.
        let acer = pill_scale_for(Some(768.0));
        assert!((acer - 768.0 / 900.0).abs() < 1e-6);
        // Shorter panels clamp at the legibility floor, monotonically.
        assert_eq!(pill_scale_for(Some(700.0)), 0.82);
        assert_eq!(pill_scale_for(Some(1.0)), 0.82);
        assert!(acer < s && s < 1.0);
    }

    /// Multi-output: the shell's own output wins over enumeration order; the
    /// first output is only the pre-enter (or post-leave) fallback.
    #[test]
    fn scale_prefers_the_output_the_shell_maps_to() {
        // Bar on the laptop panel (800) while a 1440 external enumerated
        // first: the laptop's height drives the scale.
        assert_eq!(
            preferred_output_height(Some(800.0), Some(1440.0)),
            Some(800.0)
        );
        // And the mirror case: bar on the external, laptop enumerated first.
        assert_eq!(
            preferred_output_height(Some(1440.0), Some(800.0)),
            Some(1440.0)
        );
        // No enter yet → fall back to the first output.
        assert_eq!(preferred_output_height(None, Some(768.0)), Some(768.0));
        // Nothing known at all → None, which pill_scale_for reads as full size
        // (matching the creation-time exclusive zone).
        assert_eq!(preferred_output_height(None, None), None);
        assert_eq!(pill_scale_for(preferred_output_height(None, None)), 1.0);
    }

    #[test]
    fn scaled_bar_keeps_pill_band_positive_and_proportional() {
        // The pill band (bar minus its margins) at the default 28px bar must
        // stay positive at every reachable scale, and the band shrinks by
        // strictly less than the bar (fixed margins) — so pills stay legible.
        for h in [700.0_f32, 768.0, 800.0, 900.0] {
            let s = pill_scale_for(Some(h));
            let bar = 28.0 * s;
            let band = bar - 2.0 * PILL_MARGIN_Y;
            assert!(band > 0.0, "band collapsed at h={h}");
            // Scaled text still fits the band it is centred in.
            assert!(FONT_PX * s < band + 2.0 * PILL_MARGIN_Y);
        }
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
    fn ink_is_real_black_and_white() {
        // Pure, and neutral: no channel leads (the old ink was warmed, r > g
        // > b — Max, 2026-09-12, wants the real pair).
        assert_eq!(INK_LIGHT, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(INK_DARK, [0.0, 0.0, 0.0, 1.0]);
        // Which is the widest separation there is, on either surface.
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

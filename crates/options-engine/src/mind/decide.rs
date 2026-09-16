//! The decision: `ContextState` → a ranked, de-cluttered `OptionSet`.
//!
//! This is the heart of OPTIONS as a *system*, expressed as a pure function so
//! it is fully deterministic and testable. It runs the five pillars concretely:
//!
//! - **Providers** read the context and propose affordances when their use is
//!   logical (pillar 3, "resources appear when their use is logical").
//! - **Freshness gating** drops anything whose source layer is not alive — the
//!   mind never surfaces from a dead sensor (real context-awareness).
//! - **Calibration** scales scaffolding by demonstrated skill (pillar 4,
//!   "dynamic difficulty"): help fades for experts, safety never does.
//! - **Suppression** removes the low-relevance and caps the count (pillar 3's
//!   other half, "sacar del camino" — clear away what isn't needed).
//!
//! The surface then integrates the result into the environment (pillars 1 & 5).
//!
//! # The offers are deliberately empty (2026-09-12)
//!
//! This file used to carry 32 providers emitting 76 affordances, grown by an
//! autonomous build queue that chose its own targets. Max's call: they were
//! never curated, so they go. **The machine below is intact and every table
//! that named an offer is now an empty seam** — `PROVIDERS`,
//! [`primary_modules`], [`fits_activity`], [`contextual_relevance`] and
//! [`temporal_affordances`].
//!
//! They get filled one at a time, from the curated CONTEXT list and the
//! curated OPTIONS list in `~/Golem/OPTIONS/`. The deleted providers are
//! recoverable from git (`9ed17b1`) if a curated OPTION wants the same
//! plumbing — the plumbing, not the offer, is what may be worth reusing.
//!
//! One was curated back in on 2026-09-13 — `space.empty`, the empty-room module
//! — and **removed the same day at Max's word** once he saw it on the bar. The
//! seam is empty again; the surface it fed is gone with it.

use crate::state::{ContextState, Layer};

use super::activity::{infer_activity, Activity};
use super::affordance::{Affordance, AffordanceKind, OptionSet};
use super::session::Temporal;
use super::shell::{ShellState, ShowsIn};

/// Knobs for the decision. Kept small and explicit; the `skill` term is the
/// seam where behavioural calibration (Layer 3) will feed in.
#[derive(Debug, Clone)]
pub struct Tuning {
    /// Affordances below this final relevance are cleared away.
    pub min_relevance: f32,
    /// At most this many options are ever surfaced at once (de-clutter).
    pub max_items: usize,
    /// Demonstrated competence in the current context, `0.0` (novice, wants
    /// scaffolding) … `1.0` (expert, wants it out of the way).
    ///
    /// **Still a constant everywhere it is constructed** — the daemon hands in
    /// `0.5` and nothing moves it, so only friction (see [`effective_skill`])
    /// varies the calibration today. That is finding #79.
    pub skill: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            min_relevance: 0.2,
            max_items: 3,
            skill: 0.5,
        }
    }
}

/// Every provider: a pure `(context, arrangement) → proposed affordances` rule.
/// Each sets a *base* relevance; calibration and suppression happen afterwards.
///
/// # Why a provider sees the arrangement too
///
/// Max, 2026-09-12, approving the seventeen contexts: *"we also need normal
/// tile, stage, overview, empty workspace, second monitor — all of those will
/// condition the options too."* [`fits_shell`] can only ever *suppress* on that
/// axis; an OPTION whose whole trigger IS the arrangement (the empty room) has
/// nothing to suppress, because without the arrangement it would never be
/// proposed. So the arrangement is an input to the proposal, not just a filter
/// over it.
///
/// Widened on 2026-09-13, when `PROVIDERS` was still empty — the one moment in
/// this file's life when the signature could change without migrating anything.
type Provider = fn(&ContextState, &ShellState) -> Vec<Affordance>;

/// **Empty by design.** One entry lands here per curated OPTION, as Max and the
/// technician work down the OPTIONS list — never by a queue picking its own
/// targets, and never one the technician invented. See the module docs.
const PROVIDERS: &[Provider] = &[];

/// Decide the current option set from a context snapshot alone (no temporal
/// memory). Convenience over [`decide_with`] for callers/tests without a
/// [`Session`](super::session::Session).
pub fn decide(ctx: &ContextState, tuning: &Tuning) -> OptionSet {
    decide_with(ctx, &Temporal::default(), &ShellState::default(), tuning)
}

/// Decide the current option set from a context snapshot plus the session's
/// [`Temporal`] memory (duration in activity, failure streaks) and the shell's
/// [`ShellState`] (normal tile / stage / overview, empty workspace, screens).
///
/// The two axes are both context and both condition the outcome: *what you are
/// doing* and *where you are doing it*.
pub fn decide_with(
    ctx: &ContextState,
    temporal: &Temporal,
    shell: &ShellState,
    tuning: &Tuning,
) -> OptionSet {
    let activity = infer_activity(ctx);
    let mut items: Vec<Affordance> = PROVIDERS.iter().flat_map(|p| p(ctx, shell)).collect();
    items.extend(temporal_affordances(activity, temporal));

    // Never surface from a source that isn't live (freshness gate).
    items.retain(|a| layer_alive(ctx, a.source));

    // Clear away what doesn't fit the situation — on BOTH axes.
    items.retain(|a| fits_activity(a.id, activity) && fits_shell(a.shows_in, shell));

    // Calibrate to *effective* skill (dynamic difficulty: friction lowers it),
    // then situate to the activity, then clear away the irrelevant.
    let skill = effective_skill(ctx, tuning.skill);
    let media_fg = media_is_foreground(ctx, activity);
    for a in &mut items {
        a.relevance = calibrate(a.kind, a.relevance, skill);
        a.relevance = contextual_relevance(a.id, a.relevance, activity, media_fg);
    }
    items.retain(|a| a.relevance >= tuning.min_relevance);

    // Rank and cap.
    items.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    cap_bystander_modules(&mut items, activity, media_fg);
    items.truncate(tuning.max_items);

    OptionSet {
        activity,
        items,
        generation: ctx.generation,
    }
}

/// How many CONTROLS a module that ISN'T what you're doing may put on the bar.
const BYSTANDER_MODULE_CAP: usize = 2;

/// The modules that belong to what the user is doing right now. These keep
/// their whole cluster — transport controls while a video plays, the git and
/// build controls while coding — because that cluster IS the moment.
///
/// **Empty seam.** This table is the curated CONTEXT list expressed in code:
/// for each context, which OPTION modules *are* that moment. It is filled one
/// context at a time alongside [`Activity`], which is itself still the coarse
/// eight-value enum and will grow with the list.
fn primary_modules(_activity: Activity, _media_fg: bool) -> &'static [&'static str] {
    &[]
}

/// Keep a module that is NOT the current activity from owning the bar: ranked
/// highest-first, each bystander module keeps its best
/// [`BYSTANDER_MODULE_CAP`] controls and the rest drop out so another module
/// gets a turn.
///
/// The bar has seven slots and one module used to be able to take four of them
/// — commit + push + pull + diff, git's whole menu, while the user was doing
/// something else entirely (Max, 2026-09-02: "im on btop it show me 5 options
/// for git hub"). OPTIONS suggests the right thing for the moment; it is not a
/// toolbar. What you ARE doing keeps its cluster (see [`primary_modules`]) —
/// six transport pills while you watch a video is the point, not clutter.
///
/// Warnings and Info are exempt: they are not a module competing for space,
/// they are the system telling you something (a privacy warning must never be
/// squeezed out by controls).
///
/// Inert until [`primary_modules`] has entries — with no primary there is no
/// bystander, and the early return below says so.
fn cap_bystander_modules(items: &mut Vec<Affordance>, activity: Activity, media_fg: bool) {
    let primary = primary_modules(activity, media_fg);
    // With no idea what the user is doing (Idle/Unknown) there is no bystander
    // to demote — everything on offer is equally speculative, and dropping half
    // of it at random would only make the bar less useful. Cap nothing.
    if primary.is_empty() {
        return;
    }
    let mut per_module: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    items.retain(|a| {
        if !matches!(a.kind, AffordanceKind::Control | AffordanceKind::Action) {
            return true;
        }
        let module = a.id.split('.').next().unwrap_or(a.id);
        if primary.contains(&module) {
            return true;
        }
        let n = per_module.entry(module).or_insert(0);
        *n += 1;
        *n <= BYSTANDER_MODULE_CAP
    });
}

/// Affordances that only exist with temporal memory: how long you've been at
/// something, and streaks. Pure in `(activity, temporal)`.
///
/// **Empty seam.** [`Temporal`] still carries `activity_secs` and
/// `failure_streak` — the session memory is live and measured; nothing is
/// offered from it until a curated OPTION asks for it.
fn temporal_affordances(_activity: Activity, _temporal: &Temporal) -> Vec<Affordance> {
    Vec::new()
}

/// Activity-aware suppression: some affordances are noise in some situations.
/// Safety (warnings) always fits; this only clears ambient distractions.
///
/// **Empty seam.** The rules that lived here were per-offer ("git controls
/// belong to Coding", "no media controls during a call"). They come back as
/// each curated OPTION declares the contexts it belongs to — which is exactly
/// the "Shows in:" line of the OPTIONS list.
fn fits_activity(_id: &str, _activity: Activity) -> bool {
    true
}

/// Arrangement-aware suppression: the other half of "clear away what isn't
/// needed". Where [`fits_activity`] asks *does this fit what you are doing*,
/// this asks *does this fit where you are* — normal tile, stage, overview, an
/// empty workspace, one screen or two.
///
/// The rule that is already settled (Max, 2026-09-12): **the stage ADDS to the
/// context set, the overview REPLACES it.** Every affordance declares its
/// [`ShowsIn`], and the overview shows only window-management OPTIONS — what
/// you were doing is not the question while you are looking at your windows.
///
/// Doing it here rather than in the surface means the Mind never *ranks* what
/// it would never show, so a context OPTION cannot win a slot in the overview
/// and silently displace a window-management one.
///
/// The per-OPTION refinements (`workspace_empty`, `monitors`) fill in from the
/// `Shows in:` lines as each curated OPTION is built.
fn fits_shell(shows_in: ShowsIn, _shell: &ShellState) -> bool {
    shows_in.fits(_shell.mode)
}

/// Pillar 4 (dynamic difficulty): the *effective* skill for this moment.
/// Observed friction lowers it, so scaffolding grows when the user is
/// struggling and recedes when they're fluent — all from non-invasive signals
/// (window churn, a failed command, editor diagnostics), no input capture.
fn effective_skill(ctx: &ContextState, base: f32) -> f32 {
    let churn = (ctx.behavior.focus_switch_velocity / 2.0).clamp(0.0, 1.0) * 0.4;
    let shell_err = match ctx.app_internal.shell_exit_code {
        Some(code) if code != 0 => 0.3,
        _ => 0.0,
    };
    let diagnostics = if ctx.app_internal.editor_diagnostics_count > 0 {
        0.2
    } else {
        0.0
    };
    let hesitating = if ctx.behavior.is_hesitating { 0.3 } else { 0.0 };
    let friction = (churn + shell_err + diagnostics + hesitating).clamp(0.0, 1.0);
    (base - friction).clamp(0.0, 1.0)
}

/// Situate an affordance's relevance to the current activity — a track playing
/// quietly *behind* your work should not outrank the controls for the work
/// itself.
///
/// **Empty seam.** The damping rule that lived here keyed off the `media.`
/// id namespace, which no longer exists. The principle survives and returns
/// with the curated OPTIONS that need it; [`media_is_foreground`] below is kept
/// because it answers a question about *context*, not about any one offer.
fn contextual_relevance(_id: &str, base: f32, _activity: Activity, _media_fg: bool) -> f32 {
    base
}

/// Whether media is the FOREGROUND thing — full-weight controls, as opposed to
/// a track playing quietly behind the work.
///
/// This used to carry its own browser-player table to spot a video tab. It no
/// longer needs one: [`Activity::Watching`] is *defined* as the focused window
/// being the thing that is playing, so the question is already answered one
/// layer up. Music from another window while you code classifies as `Coding`,
/// which is exactly the damped case.
fn media_is_foreground(_ctx: &ContextState, activity: Activity) -> bool {
    activity == Activity::Watching
}

/// Skill scaling: safety and direct controls are untouchable; only scaffolding
/// and ambient info fade for experts.
fn calibrate(kind: AffordanceKind, relevance: f32, skill: f32) -> f32 {
    let factor = match kind {
        // Safety is always relevant when true.
        AffordanceKind::Warning => 1.0,
        // A direct control (play/pause, mute, commit) is the button you reached
        // for — an expert wants it as much as a novice. Never faded.
        AffordanceKind::Control => 1.0,
        // Scaffolding/help fades most for experts.
        AffordanceKind::Action => 1.0 - 0.6 * skill,
        // Ambient info fades gently.
        AffordanceKind::Info => 1.0 - 0.3 * skill,
    };
    (relevance * factor).clamp(0.0, 1.0)
}

/// Whether the layer feeding an affordance is currently alive.
fn layer_alive(ctx: &ContextState, layer: Layer) -> bool {
    match layer {
        Layer::Compositor => ctx.health.compositor.alive,
        Layer::Selection => ctx.health.selection.alive,
        Layer::AppBridge => ctx.health.app_bridge.alive,
        Layer::Behavior => ctx.health.behavior.alive,
        Layer::Hardware => ctx.health.hardware.alive,
        Layer::System => ctx.health.system.alive,
        Layer::Notifications => ctx.health.notifications.alive,
        Layer::Daylight => ctx.health.daylight.alive,
        Layer::Bluetooth => ctx.health.bluetooth.alive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::affordance::AffordanceAction;

    /// A hand-built affordance, so the machine can be tested without any
    /// providers existing.
    fn aff(id: &'static str, kind: AffordanceKind, relevance: f32, source: Layer) -> Affordance {
        Affordance {
            id,
            kind,
            title: id.into(),
            detail: String::new(),
            relevance,
            reason: "test",
            source,
            action: AffordanceAction::None,
            immediate: false,
            shows_in: ShowsIn::Context,
        }
    }

    fn live_ctx() -> ContextState {
        let mut ctx = ContextState::default();
        ctx.health.compositor.alive = true;
        ctx.health.hardware.alive = true;
        ctx.health.behavior.alive = true;
        ctx.health.app_bridge.alive = true;
        ctx.health.selection.alive = true;
        ctx.health.system.alive = true;
        ctx.health.notifications.alive = true;
        ctx.health.daylight.alive = true;
        ctx
    }

    #[test]
    fn no_providers_means_no_offers() {
        // The state this file is deliberately in: the machine runs, decides
        // nothing, and says so. This test fails the moment a provider is added
        // — which is the reminder to add its curated-list entry too.
        let set = decide(&live_ctx(), &Tuning::default());
        assert!(
            set.items.is_empty(),
            "PROVIDERS is empty by design (see the module docs); \
             adding one means adding its OPTIONS-list entry"
        );
    }

    #[test]
    fn calibrate_scales_scaffolding_by_skill_but_never_safety() {
        // Pillar 4's whole contract, in four lines.
        let novice = 0.0;
        let expert = 1.0;

        // Scaffolding fades most.
        assert!(
            calibrate(AffordanceKind::Action, 1.0, expert)
                < calibrate(AffordanceKind::Action, 1.0, novice)
        );
        // Ambient info fades gently — and less than scaffolding does.
        assert!(
            calibrate(AffordanceKind::Info, 1.0, expert)
                < calibrate(AffordanceKind::Info, 1.0, novice)
        );
        assert!(
            calibrate(AffordanceKind::Action, 1.0, expert)
                < calibrate(AffordanceKind::Info, 1.0, expert)
        );
        // Controls and warnings are untouchable at any skill.
        assert_eq!(calibrate(AffordanceKind::Control, 1.0, expert), 1.0);
        assert_eq!(calibrate(AffordanceKind::Warning, 1.0, expert), 1.0);
    }

    #[test]
    fn friction_lowers_effective_skill_so_help_comes_back() {
        let calm = live_ctx();
        assert_eq!(effective_skill(&calm, 1.0), 1.0);

        let mut struggling = live_ctx();
        struggling.app_internal.shell_exit_code = Some(1);
        struggling.app_internal.editor_diagnostics_count = 3;
        struggling.behavior.is_hesitating = true;
        assert!(
            effective_skill(&struggling, 1.0) < 1.0,
            "observed friction must lower effective skill (pillar 4)"
        );
    }

    #[test]
    fn a_dead_source_never_surfaces() {
        let mut ctx = live_ctx();
        ctx.health.system.alive = false;
        assert!(!layer_alive(&ctx, Layer::System));
        assert!(layer_alive(&ctx, Layer::Compositor));
    }

    #[test]
    fn the_cap_is_inert_while_primary_modules_is_empty() {
        // Documents the current seam state: with no context→module table there
        // is no bystander, so nothing is demoted. Fill `primary_modules` and
        // this expectation must be rewritten along with it.
        let mut items = vec![
            aff("a.one", AffordanceKind::Control, 0.9, Layer::System),
            aff("a.two", AffordanceKind::Control, 0.8, Layer::System),
            aff("a.three", AffordanceKind::Control, 0.7, Layer::System),
        ];
        cap_bystander_modules(&mut items, Activity::Coding, false);
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn foreground_media_is_exactly_the_watching_context() {
        let ctx = live_ctx();
        assert!(media_is_foreground(&ctx, Activity::Watching));
        // Music behind the work is the damped case, whatever the work is.
        for background in [Activity::Coding, Activity::Browsing, Activity::Drawing] {
            assert!(!media_is_foreground(&ctx, background), "{background:?}");
        }
    }

    #[test]
    fn the_arrangement_reaches_the_decision() {
        // Both axes arrive at `decide_with`; with no providers the set is empty
        // either way, but the plumbing must exist before an OPTION can read it.
        let ctx = live_ctx();
        let stage = ShellState {
            mode: super::super::shell::ShellMode::Stage,
            monitors: 2,
            ..Default::default()
        };
        let set = decide_with(&ctx, &Temporal::default(), &stage, &Tuning::default());
        assert!(set.items.is_empty());
        assert!(stage.has_second_screen());
    }

    /// The suppression half of Max's rule, exercised through the real filter
    /// rather than through `ShowsIn::fits` alone: a context OPTION must not
    /// survive the decision in the overview.
    #[test]
    fn a_context_option_is_dropped_by_the_overview() {
        use super::super::shell::ShellMode;
        let context_option = aff("ctx.one", AffordanceKind::Control, 0.9, Layer::System);
        let overview_option = Affordance {
            shows_in: ShowsIn::Overview,
            ..aff("ov.one", AffordanceKind::Control, 0.9, Layer::System)
        };

        for (mode, want_ctx, want_ov) in [
            (ShellMode::NormalTile, true, false),
            (ShellMode::Stage, true, false),
            (ShellMode::Overview, false, true),
        ] {
            let shell = ShellState {
                mode,
                ..Default::default()
            };
            assert_eq!(
                fits_shell(context_option.shows_in, &shell),
                want_ctx,
                "context OPTION under {mode:?}"
            );
            assert_eq!(
                fits_shell(overview_option.shows_in, &shell),
                want_ov,
                "overview OPTION under {mode:?}"
            );
        }
    }
}

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

use crate::state::{ContextState, Layer};

use super::activity::{infer_activity, Activity};
use super::affordance::{Affordance, AffordanceKind, OptionSet};

/// Knobs for the decision. Kept small and explicit; the `skill` term is the
/// seam where behavioural calibration (Layer 3) will feed in.
#[derive(Debug, Clone)]
pub struct Tuning {
    /// Affordances below this final relevance are cleared away.
    pub min_relevance: f32,
    /// At most this many options are ever surfaced at once (de-clutter).
    pub max_items: usize,
    /// Demonstrated competence in the current context, `0.0` (novice, wants
    /// scaffolding) … `1.0` (expert, wants it out of the way). Fed by Layer 3
    /// later; a neutral default for now.
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

/// Every provider: a pure `context → proposed affordances` rule. Each sets a
/// *base* relevance; calibration and suppression happen afterwards.
type Provider = fn(&ContextState) -> Vec<Affordance>;

const PROVIDERS: &[Provider] = &[
    screencast_provider,
    battery_provider,
    cpu_provider,
    media_provider,
    git_provider,
    focus_churn_provider,
    shell_error_provider,
    diagnostics_provider,
    selection_provider,
    mic_provider,
];

/// Decide the current option set from a context snapshot.
pub fn decide(ctx: &ContextState, tuning: &Tuning) -> OptionSet {
    let activity = infer_activity(ctx);
    let mut items: Vec<Affordance> = PROVIDERS.iter().flat_map(|p| p(ctx)).collect();

    // Never surface from a source that isn't live (freshness gate).
    items.retain(|a| layer_alive(ctx, a.source));

    // Clear away what doesn't fit the situation (activity-aware declutter).
    items.retain(|a| fits_activity(a.id, activity));

    // Calibrate to *effective* skill (dynamic difficulty: friction lowers it),
    // then clear away the irrelevant.
    let skill = effective_skill(ctx, tuning.skill);
    for a in &mut items {
        a.relevance = calibrate(a.kind, a.relevance, skill);
    }
    items.retain(|a| a.relevance >= tuning.min_relevance);

    // Rank and cap.
    items.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(tuning.max_items);

    OptionSet {
        activity,
        items,
        generation: ctx.generation,
    }
}

/// Activity-aware suppression: some affordances are noise in some situations.
/// Safety (warnings) always fits; this only clears ambient distractions.
fn fits_activity(id: &str, activity: Activity) -> bool {
    match activity {
        // In a call, now-playing media is a distraction — clear it away.
        Activity::Communication => id != "media.now_playing",
        _ => true,
    }
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

/// Skill scaling: safety is untouchable; scaffolding fades for experts.
fn calibrate(kind: AffordanceKind, relevance: f32, skill: f32) -> f32 {
    let factor = match kind {
        AffordanceKind::Warning => 1.0,
        AffordanceKind::Action => 1.0 - 0.6 * skill,
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
    }
}

// --- Providers -------------------------------------------------------------

/// Sharing your screen is a "right moment" par excellence — surface it clearly.
fn screencast_provider(ctx: &ContextState) -> Vec<Affordance> {
    if !ctx.is_screencasting {
        return vec![];
    }
    vec![Affordance {
        id: "compositor.screencasting",
        kind: AffordanceKind::Warning,
        title: "Screen is being shared".into(),
        detail: "A screencast or share is live".into(),
        relevance: 0.9,
        reason: "screencast active",
        source: Layer::Compositor,
    }]
}

/// Battery: escalating urgency as it drains on battery power.
fn battery_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(pct) = ctx.metrics.battery_pct else {
        return vec![];
    };
    if ctx.metrics.is_charging {
        return vec![];
    }
    if pct <= 15 {
        vec![Affordance {
            id: "system.battery_critical",
            kind: AffordanceKind::Warning,
            title: format!("Battery low — {pct}%"),
            detail: "Plug in soon".into(),
            relevance: 0.97,
            reason: "battery <=15% on power",
            source: Layer::Hardware,
        }]
    } else if pct <= 30 {
        vec![Affordance {
            id: "system.battery_low",
            kind: AffordanceKind::Info,
            title: format!("Battery at {pct}%"),
            detail: "Running on battery".into(),
            relevance: 0.5,
            reason: "battery <=30% on power",
            source: Layer::Hardware,
        }]
    } else {
        vec![]
    }
}

/// Sustained high CPU is worth a quiet note.
fn cpu_provider(ctx: &ContextState) -> Vec<Affordance> {
    if ctx.metrics.cpu_usage_pct < 85.0 {
        return vec![];
    }
    vec![Affordance {
        id: "system.high_cpu",
        kind: AffordanceKind::Info,
        title: "High CPU load".into(),
        detail: format!("{:.0}% in use", ctx.metrics.cpu_usage_pct),
        relevance: 0.4,
        reason: "cpu >=85%",
        source: Layer::Hardware,
    }]
}

/// What you're listening to, while it's playing.
fn media_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(m) = &ctx.media else {
        return vec![];
    };
    let detail = match (m.title.is_empty(), m.artist.is_empty()) {
        (false, false) => format!("{} — {}", m.title, m.artist),
        (false, true) => m.title.clone(),
        _ => m.player_name.clone(),
    };
    vec![Affordance {
        id: "media.now_playing",
        kind: AffordanceKind::Info,
        title: if m.is_playing { "Now playing" } else { "Paused" }.into(),
        detail,
        // A playing track is more present than a paused one.
        relevance: if m.is_playing { 0.45 } else { 0.25 },
        reason: "mpris player present",
        source: Layer::Hardware,
    }]
}

/// The repository you're working in (and, later, whether it's dirty).
fn git_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(branch) = &ctx.git.branch else {
        return vec![];
    };
    let repo = ctx
        .git
        .repo_root
        .as_deref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = vec![Affordance {
        id: "git.branch",
        kind: AffordanceKind::Info,
        title: format!("On {branch}"),
        detail: repo,
        relevance: 0.35,
        reason: "focused window in a git repo",
        source: Layer::Hardware,
    }];
    if ctx.git.is_dirty {
        out.push(Affordance {
            id: "git.dirty",
            kind: AffordanceKind::Warning,
            title: "Uncommitted changes".into(),
            detail: format!("on {branch}"),
            relevance: 0.6,
            reason: "working tree dirty",
            source: Layer::Hardware,
        });
    }
    out
}

/// Behavioural: rapid window churn suggests searching/losing the thread — a
/// gentle scaffolding cue (fades for experts via calibration). Reading actions
/// and reacting is pillar 2; this is a first, non-invasive expression of it.
fn focus_churn_provider(ctx: &ContextState) -> Vec<Affordance> {
    let vel = ctx.behavior.focus_switch_velocity;
    if vel < 0.6 {
        return vec![];
    }
    vec![Affordance {
        id: "behavior.focus_churn",
        kind: AffordanceKind::Action,
        title: "Lots of window switching".into(),
        detail: "Jump to a window or search?".into(),
        relevance: (vel * 0.4).clamp(0.0, 0.7),
        reason: "high focus-switch velocity",
        source: Layer::Compositor,
    }]
}

/// A shell command that just failed — the moment a helpful hand turns
/// frustration into revelation. Scaffolding (an Action), so it fades for
/// experts and grows for strugglers.
fn shell_error_provider(ctx: &ContextState) -> Vec<Affordance> {
    match ctx.app_internal.shell_exit_code {
        Some(code) if code != 0 => {
            let detail = ctx
                .app_internal
                .shell_last_cmd
                .as_deref()
                .map(|c| format!("{c} → exit {code}"))
                .unwrap_or_else(|| format!("exit {code}"));
            vec![Affordance {
                id: "shell.last_failed",
                kind: AffordanceKind::Action,
                title: "Last command failed".into(),
                detail,
                relevance: 0.55,
                reason: "nonzero shell exit code",
                source: Layer::AppBridge,
            }]
        }
        _ => vec![],
    }
}

/// Diagnostics in the focused editor buffer — surface the count while you work.
fn diagnostics_provider(ctx: &ContextState) -> Vec<Affordance> {
    let n = ctx.app_internal.editor_diagnostics_count;
    if n == 0 {
        return vec![];
    }
    let file = ctx
        .app_internal
        .editor_file
        .as_deref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    vec![Affordance {
        id: "editor.diagnostics",
        kind: AffordanceKind::Info,
        title: format!("{n} problem{}", if n == 1 { "" } else { "s" }),
        detail: file,
        // A little more pressing as they pile up.
        relevance: (0.35 + 0.05 * n.min(5) as f32).clamp(0.0, 0.6),
        reason: "editor diagnostics present",
        source: Layer::AppBridge,
    }]
}

/// A live microphone — awareness that you're being heard (and the seat of the
/// Communication activity).
fn mic_provider(ctx: &ContextState) -> Vec<Affordance> {
    if !ctx.audio.is_mic_active {
        return vec![];
    }
    vec![Affordance {
        id: "audio.mic_live",
        kind: AffordanceKind::Warning,
        title: "Microphone is live".into(),
        detail: "You're being recorded or heard".into(),
        relevance: 0.8,
        reason: "active capture stream",
        source: Layer::Hardware,
    }]
}

/// A copied URL or code snippet is strong intent — offer to act on it.
fn selection_provider(ctx: &ContextState) -> Vec<Affordance> {
    let s = &ctx.selection;
    if s.char_count == 0 {
        return vec![];
    }
    if s.contains_url {
        vec![Affordance {
            id: "selection.url",
            kind: AffordanceKind::Action,
            title: "Open copied link".into(),
            detail: s.highlighted_text.clone().unwrap_or_default(),
            relevance: 0.5,
            reason: "clipboard holds a url",
            source: Layer::Selection,
        }]
    } else if s.is_code {
        vec![Affordance {
            id: "selection.code",
            kind: AffordanceKind::Action,
            title: "Copied code".into(),
            detail: format!("{} characters", s.char_count),
            relevance: 0.4,
            reason: "clipboard looks like code",
            source: Layer::Selection,
        }]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppInternalContext, GitContext, MediaState, SystemMetrics, TextSelection};

    /// A context with everything's source layer marked alive, so freshness
    /// gating doesn't hide provider output under test.
    fn live_ctx() -> ContextState {
        let mut ctx = ContextState::default();
        ctx.health.compositor.alive = true;
        ctx.health.hardware.alive = true;
        ctx.health.behavior.alive = true;
        ctx.health.app_bridge.alive = true;
        ctx.health.selection.alive = true;
        ctx
    }

    #[test]
    fn surfaces_nothing_from_empty_context() {
        let opts = decide(&live_ctx(), &Tuning::default());
        assert!(opts.items.is_empty());
    }

    #[test]
    fn screencast_is_a_high_warning() {
        let mut ctx = live_ctx();
        ctx.is_screencasting = true;
        let opts = decide(&ctx, &Tuning::default());
        assert_eq!(opts.items[0].id, "compositor.screencasting");
        assert_eq!(opts.items[0].kind, AffordanceKind::Warning);
    }

    #[test]
    fn dead_source_is_never_surfaced() {
        let mut ctx = live_ctx();
        ctx.media = Some(MediaState {
            player_name: "x".into(),
            title: "t".into(),
            artist: "a".into(),
            is_playing: true,
        });
        // With hardware alive it shows…
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .any(|a| a.id == "media.now_playing"));
        // …but if the hardware layer is dead, it must not.
        ctx.health.hardware.alive = false;
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| a.id != "media.now_playing"));
    }

    #[test]
    fn warnings_outrank_info_and_cap_applies() {
        let mut ctx = live_ctx();
        ctx.is_screencasting = true;
        ctx.metrics = SystemMetrics {
            cpu_usage_pct: 95.0,
            battery_pct: Some(10),
            is_charging: false,
            ..Default::default()
        };
        ctx.media = Some(MediaState {
            player_name: "p".into(),
            title: "t".into(),
            artist: "a".into(),
            is_playing: true,
        });
        ctx.git = GitContext {
            branch: Some("main".into()),
            ..Default::default()
        };
        let tuning = Tuning {
            max_items: 2,
            ..Default::default()
        };
        let opts = decide(&ctx, &tuning);
        assert_eq!(opts.items.len(), 2, "capped to max_items");
        // The two warnings (battery critical, screencast) win the top slots.
        assert!(opts.items.iter().all(|a| a.kind == AffordanceKind::Warning));
        // Sorted descending.
        assert!(opts.items[0].relevance >= opts.items[1].relevance);
    }

    #[test]
    fn shell_failure_surfaces_as_actionable_help() {
        let mut ctx = live_ctx();
        ctx.app_internal = AppInternalContext {
            shell_last_cmd: Some("cargo build".into()),
            shell_exit_code: Some(101),
            ..Default::default()
        };
        let opts = decide(&ctx, &Tuning::default());
        let a = opts
            .items
            .iter()
            .find(|a| a.id == "shell.last_failed")
            .expect("failure should surface");
        assert_eq!(a.kind, AffordanceKind::Action);
        assert!(a.detail.contains("cargo build"));
        // A zero exit surfaces nothing.
        ctx.app_internal.shell_exit_code = Some(0);
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| a.id != "shell.last_failed"));
    }

    #[test]
    fn copied_url_offers_to_open() {
        let mut ctx = live_ctx();
        ctx.selection = TextSelection {
            highlighted_text: Some("https://example.com".into()),
            char_count: 19,
            is_code: false,
            contains_url: true,
        };
        let opts = decide(&ctx, &Tuning::default());
        assert!(opts.items.iter().any(|a| a.id == "selection.url"));
    }

    #[test]
    fn diagnostics_surface_and_need_a_live_bridge() {
        let mut ctx = live_ctx();
        ctx.app_internal = AppInternalContext {
            editor_file: Some("/p/src/main.rs".into()),
            editor_diagnostics_count: 3,
            ..Default::default()
        };
        let opts = decide(&ctx, &Tuning::default());
        let a = opts
            .items
            .iter()
            .find(|a| a.id == "editor.diagnostics")
            .expect("diagnostics should surface");
        assert!(a.title.contains('3') && a.detail == "main.rs");
        // If the bridge layer is dead, its affordances vanish (freshness gate).
        ctx.health.app_bridge.alive = false;
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| a.id != "editor.diagnostics"));
    }

    #[test]
    fn calibrate_scales_scaffolding_by_skill_but_never_safety() {
        // Safety is untouchable regardless of skill.
        assert_eq!(calibrate(AffordanceKind::Warning, 0.9, 1.0), 0.9);
        assert_eq!(calibrate(AffordanceKind::Warning, 0.9, 0.0), 0.9);
        // An Action fades for the expert, full for the novice.
        assert!(calibrate(AffordanceKind::Action, 0.4, 1.0) < calibrate(AffordanceKind::Action, 0.4, 0.0));
        // Below threshold for an expert, above for a novice (suppression).
        assert!(calibrate(AffordanceKind::Action, 0.4, 1.0) < 0.2);
        assert!(calibrate(AffordanceKind::Action, 0.4, 0.0) >= 0.2);
    }

    #[test]
    fn friction_lowers_effective_skill() {
        let calm = live_ctx();
        assert_eq!(effective_skill(&calm, 1.0), 1.0);

        let mut churny = live_ctx();
        churny.behavior.focus_switch_velocity = 2.0; // max churn contribution
        assert!(effective_skill(&churny, 1.0) < 1.0);

        let mut failing = live_ctx();
        failing.app_internal.shell_exit_code = Some(1);
        failing.app_internal.editor_diagnostics_count = 4;
        // Errors + diagnostics stack, pulling effective skill well down.
        assert!(effective_skill(&failing, 1.0) <= 0.5);
    }

    #[test]
    fn dynamic_difficulty_keeps_help_under_friction_and_safety_always() {
        let mut ctx = live_ctx();
        ctx.behavior.focus_switch_velocity = 2.0; // friction + focus-churn Action
        ctx.is_screencasting = true; // safety Warning

        // Even with an expert *base* skill, friction keeps the scaffolding up…
        let opts = decide(
            &ctx,
            &Tuning {
                skill: 1.0,
                ..Default::default()
            },
        );
        assert!(opts.items.iter().any(|a| a.id == "behavior.focus_churn"));
        // …and safety is present no matter what.
        assert!(opts.items.iter().any(|a| a.id == "compositor.screencasting"));
    }

    #[test]
    fn expert_sees_ambient_info_dimmer_than_novice() {
        let mut ctx = live_ctx();
        ctx.media = Some(MediaState {
            player_name: "p".into(),
            title: "t".into(),
            artist: "a".into(),
            is_playing: true,
        });
        let rel = |skill| {
            decide(&ctx, &Tuning { skill, ..Default::default() })
                .items
                .iter()
                .find(|a| a.id == "media.now_playing")
                .map(|a| a.relevance)
                .unwrap()
        };
        assert!(rel(1.0) < rel(0.0));
    }
}

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
];

/// Decide the current option set from a context snapshot.
pub fn decide(ctx: &ContextState, tuning: &Tuning) -> OptionSet {
    let mut items: Vec<Affordance> = PROVIDERS.iter().flat_map(|p| p(ctx)).collect();

    // Never surface from a source that isn't live (freshness gate).
    items.retain(|a| layer_alive(ctx, a.source));

    // Calibrate to skill, then clear away the irrelevant.
    for a in &mut items {
        a.relevance = calibrate(a.kind, a.relevance, tuning.skill);
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
        items,
        generation: ctx.generation,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GitContext, MediaState, SystemMetrics};

    /// A context with everything's source layer marked alive, so freshness
    /// gating doesn't hide provider output under test.
    fn live_ctx() -> ContextState {
        let mut ctx = ContextState::default();
        ctx.health.compositor.alive = true;
        ctx.health.hardware.alive = true;
        ctx.health.behavior.alive = true;
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
    fn expert_skill_suppresses_scaffolding_but_not_safety() {
        let mut ctx = live_ctx();
        ctx.behavior.focus_switch_velocity = 1.0; // Action affordance
        ctx.is_screencasting = true; // Warning affordance

        let novice = decide(
            &ctx,
            &Tuning {
                skill: 0.0,
                ..Default::default()
            },
        );
        assert!(novice.items.iter().any(|a| a.id == "behavior.focus_churn"));

        let expert = decide(
            &ctx,
            &Tuning {
                skill: 1.0,
                ..Default::default()
            },
        );
        // Scaffolding is calibrated away for the expert…
        assert!(expert.items.iter().all(|a| a.id != "behavior.focus_churn"));
        // …but the safety warning still stands.
        assert!(expert.items.iter().any(|a| a.id == "compositor.screencasting"));
    }
}

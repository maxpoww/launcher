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
use super::affordance::{Affordance, AffordanceAction, AffordanceKind, OptionSet};
use super::session::Temporal;

/// A coding stretch past this long earns a gentle "take a break" nudge.
const LONG_CODING_SECS: u64 = 60 * 60;

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
    deploy_provider,
    battery_provider,
    cpu_provider,
    media_provider,
    media_controls_provider,
    git_provider,
    focus_churn_provider,
    shell_error_provider,
    diagnostics_provider,
    selection_provider,
    mic_provider,
    notifications_provider,
];

/// Decide the current option set from a context snapshot alone (no temporal
/// memory). Convenience over [`decide_with`] for callers/tests without a
/// [`Session`](super::session::Session).
pub fn decide(ctx: &ContextState, tuning: &Tuning) -> OptionSet {
    decide_with(ctx, &Temporal::default(), tuning)
}

/// Decide the current option set from a context snapshot plus the session's
/// [`Temporal`] memory (duration in activity, failure streaks).
pub fn decide_with(ctx: &ContextState, temporal: &Temporal, tuning: &Tuning) -> OptionSet {
    let activity = infer_activity(ctx);
    let mut items: Vec<Affordance> = PROVIDERS.iter().flat_map(|p| p(ctx)).collect();
    items.extend(temporal_affordances(activity, temporal));

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

/// Affordances that only exist with temporal memory: how long you've been at
/// something, and streaks. Pure in `(activity, temporal)`.
fn temporal_affordances(activity: Activity, temporal: &Temporal) -> Vec<Affordance> {
    let mut out = Vec::new();
    if activity == Activity::Coding && temporal.activity_secs >= LONG_CODING_SECS {
        out.push(Affordance {
            id: "session.long_coding",
            kind: AffordanceKind::Info,
            title: "Long coding session".into(),
            detail: format!("{} min — a break?", temporal.activity_secs / 60),
            relevance: 0.4,
            reason: "coding over an hour",
            source: Layer::Compositor,
            action: AffordanceAction::None,
        });
    }
    if temporal.failure_streak >= 3 {
        out.push(Affordance {
            id: "session.failure_streak",
            kind: AffordanceKind::Action,
            title: "Several commands failing".into(),
            detail: format!("{} in a row — want a hand?", temporal.failure_streak),
            relevance: 0.7,
            reason: "repeated shell failures",
            source: Layer::AppBridge,
            action: AffordanceAction::None,
        });
    }
    out
}

/// Activity-aware suppression: some affordances are noise in some situations.
/// Safety (warnings) always fits; this only clears ambient distractions.
fn fits_activity(id: &str, activity: Activity) -> bool {
    // git commit/push are Coding controls — surface them only while actually
    // working in the repo, not when a browser or media app merely happens to be
    // focused inside a repo directory.
    if matches!(id, "git.commit" | "git.push") {
        return activity == Activity::Coding;
    }
    match activity {
        // In a call, now-playing media and its controls are a distraction —
        // clear them away (the mic-mute control stays; it belongs to the call).
        Activity::Communication => id != "media.now_playing" && !id.starts_with("media."),
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
        action: AffordanceAction::None,
    }]
}

/// Deploy health: the system you're running isn't the system you last built.
/// Invisible-but-wrong state made visible at the moment it matters (a Warning,
/// never suppressed by skill). The two cases are mutually exclusive in what we
/// surface — a failed activation is the root cause, so it wins over the merely
/// stale generation it also implies, keeping the surface to one clear option.
fn deploy_provider(ctx: &ContextState) -> Vec<Affordance> {
    let d = &ctx.deploy;
    if d.not_activated {
        return vec![Affordance {
            id: "deploy.not_activated",
            kind: AffordanceKind::Warning,
            title: "Last deploy didn't activate".into(),
            detail: "Newest build isn't the running system — re-run switch".into(),
            relevance: 0.85,
            reason: "current system != latest built generation",
            source: Layer::System,
            action: AffordanceAction::None,
        }];
    }
    if d.stale_generation {
        return vec![Affordance {
            id: "deploy.stale_generation",
            kind: AffordanceKind::Warning,
            title: "Running a stale system generation".into(),
            detail: "Reboot to run the latest build".into(),
            relevance: 0.8,
            reason: "booted system != latest built generation",
            source: Layer::System,
            action: AffordanceAction::None,
        }];
    }
    vec![]
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
            action: AffordanceAction::None,
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
            action: AffordanceAction::None,
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
    // Sustained high CPU: surface a control to SEE what's using it (open a
    // system monitor in a terminal). A Control — a direct "show me", equally
    // useful to novice and expert. `foot btop` (both ship on Golem).
    vec![Affordance {
        id: "system.high_cpu",
        kind: AffordanceKind::Control,
        title: "High CPU".into(),
        detail: format!("{:.0}% — open monitor", ctx.metrics.cpu_usage_pct),
        relevance: 0.4,
        reason: "cpu >=85%",
        source: Layer::Hardware,
        action: spawn(&["foot", "btop"]),
    }]
}

/// Build a fire-and-forget [`AffordanceAction::Spawn`] from a static argv.
fn spawn(argv: &[&str]) -> AffordanceAction {
    AffordanceAction::Spawn {
        argv: argv.iter().map(|s| s.to_string()).collect(),
    }
}

/// The default audio sink / source handles for `wpctl`.
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// **media-controls module** — the "watching a video / listening" controls:
/// play/pause, volume, brightness, track skip. Surfaces whenever an MPRIS
/// player is present (playing or paused, so Pause↔Play both work). Every offer
/// is a [`AffordanceKind::Control`] (never faded by skill) whose action is a
/// one-shot CLI already on a Golem system (playerctl / wpctl / brightnessctl).
/// Relevance is set in the natural reading order so the cluster reads
/// left-to-right sensibly after the mind's descending sort.
fn media_controls_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(m) = &ctx.media else {
        return vec![];
    };
    let (toggle_title, toggle_reason) = if m.is_playing {
        ("Pause", "pause the playing media")
    } else {
        ("Play", "resume the paused media")
    };
    let mut out = vec![
        Affordance {
            id: "media.playpause",
            kind: AffordanceKind::Control,
            title: toggle_title.into(),
            detail: String::new(),
            relevance: 0.66,
            reason: toggle_reason,
            source: Layer::Hardware,
            action: spawn(&["playerctl", "play-pause"]),
        },
        Affordance {
            id: "media.vol_down",
            kind: AffordanceKind::Control,
            title: "Volume −".into(),
            detail: String::new(),
            relevance: 0.62,
            reason: "lower the volume",
            source: Layer::Hardware,
            action: spawn(&["wpctl", "set-volume", SINK, "5%-"]),
        },
        Affordance {
            id: "media.vol_up",
            kind: AffordanceKind::Control,
            title: "Volume +".into(),
            detail: String::new(),
            relevance: 0.61,
            reason: "raise the volume",
            source: Layer::Hardware,
            action: spawn(&["wpctl", "set-volume", "-l", "1.5", SINK, "5%+"]),
        },
        Affordance {
            id: "media.seek_back",
            kind: AffordanceKind::Control,
            title: "Back 10s".into(),
            detail: String::new(),
            relevance: 0.60,
            reason: "seek backward 10 seconds",
            source: Layer::Hardware,
            action: spawn(&["playerctl", "position", "10-"]),
        },
        Affordance {
            id: "media.seek_fwd",
            kind: AffordanceKind::Control,
            title: "Forward 10s".into(),
            detail: String::new(),
            relevance: 0.59,
            reason: "seek forward 10 seconds",
            source: Layer::Hardware,
            action: spawn(&["playerctl", "position", "10+"]),
        },
        Affordance {
            id: "media.next",
            kind: AffordanceKind::Control,
            title: "Next".into(),
            detail: String::new(),
            relevance: 0.50,
            reason: "skip to the next track",
            source: Layer::Hardware,
            action: spawn(&["playerctl", "next"]),
        },
        Affordance {
            id: "media.prev",
            kind: AffordanceKind::Control,
            title: "Previous".into(),
            detail: String::new(),
            relevance: 0.49,
            reason: "back to the previous track",
            source: Layer::Hardware,
            action: spawn(&["playerctl", "previous"]),
        },
    ];
    // Brightness is a screen control, not a media one — offer it (for
    // watching video) only where a backlight actually exists (a laptop
    // panel), so a desktop/VM never shows dead brightness pills. It outranks
    // track-skip so it leads the cluster when present.
    if ctx.metrics.has_backlight {
        out.push(Affordance {
            id: "media.bright_down",
            kind: AffordanceKind::Control,
            title: "Brightness −".into(),
            detail: String::new(),
            relevance: 0.56,
            reason: "dim the screen",
            source: Layer::Hardware,
            action: spawn(&["brightnessctl", "set", "10%-"]),
        });
        out.push(Affordance {
            id: "media.bright_up",
            kind: AffordanceKind::Control,
            title: "Brightness +".into(),
            detail: String::new(),
            relevance: 0.55,
            reason: "brighten the screen",
            source: Layer::Hardware,
            action: spawn(&["brightnessctl", "set", "10%+"]),
        });
    }
    out
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
        title: if m.is_playing {
            "Now playing"
        } else {
            "Paused"
        }
        .into(),
        detail,
        // A playing track is more present than a paused one.
        relevance: if m.is_playing { 0.45 } else { 0.25 },
        reason: "mpris player present",
        source: Layer::Hardware,
        action: AffordanceAction::None,
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
        action: AffordanceAction::None,
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
            action: AffordanceAction::None,
        });
        // **git-actions module** — the coding controls. `-C <root>` runs git in
        // the repo without a shell, so no quoting/injection surface (the root
        // comes from /proc, never user text). Commit stages tracked changes
        // (`-am`); Push publishes. Gated to the Coding activity in
        // `fits_activity` so they only appear while you're actually working in
        // the repo, not when a browser happens to be focused in a repo dir.
        if let Some(root) = ctx.git.repo_root.as_deref().and_then(|p| p.to_str()) {
            out.push(Affordance {
                id: "git.commit",
                kind: AffordanceKind::Control,
                title: "Commit all".into(),
                detail: format!("on {branch}"),
                relevance: 0.64,
                reason: "commit the tracked changes",
                source: Layer::Hardware,
                action: spawn(&["git", "-C", root, "commit", "-am", "Update (via OPTIONS)"]),
            });
            out.push(Affordance {
                id: "git.push",
                kind: AffordanceKind::Control,
                title: "Push".into(),
                detail: format!("on {branch}"),
                relevance: 0.6,
                reason: "push the branch to its remote",
                source: Layer::Hardware,
                action: spawn(&["git", "-C", root, "push"]),
            });
        }
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
        action: AffordanceAction::None,
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
                action: AffordanceAction::None,
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
        action: AffordanceAction::None,
    }]
}

/// A live microphone — awareness that you're being heard (and the seat of the
/// Communication activity).
fn mic_provider(ctx: &ContextState) -> Vec<Affordance> {
    if !ctx.audio.is_mic_active {
        return vec![];
    }
    vec![
        Affordance {
            id: "audio.mic_live",
            kind: AffordanceKind::Warning,
            title: "Microphone is live".into(),
            detail: "You're being recorded or heard".into(),
            relevance: 0.8,
            reason: "active capture stream",
            source: Layer::Hardware,
            action: AffordanceAction::None,
        },
        // **call-controls module** — the moment the mic goes live (a call), the
        // one control everyone reaches for: mute. A direct Control, so it never
        // fades; toggles the default source via wpctl.
        Affordance {
            id: "audio.mic_mute",
            kind: AffordanceKind::Control,
            title: "Mute mic".into(),
            detail: String::new(),
            relevance: 0.82,
            reason: "toggle the microphone mute",
            source: Layer::Hardware,
            action: spawn(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"]),
        },
    ]
}

/// Unread notifications the daemon is holding. A critical one is a genuine
/// "right moment" (a Warning, never suppressed by skill); a backlog of ordinary
/// unread ones is quiet ambient Info that fades for experts. The notification
/// OPTION itself owns the full list — this is only the mind's nudge that there's
/// something worth a glance.
fn notifications_provider(ctx: &ContextState) -> Vec<Affordance> {
    let n = &ctx.notifications;
    if n.active_count == 0 {
        return vec![];
    }
    let detail = match (n.latest_app.is_empty(), n.latest_summary.is_empty()) {
        (false, false) => format!("{} — {}", n.latest_app, n.latest_summary),
        (false, true) => n.latest_app.clone(),
        (true, false) => n.latest_summary.clone(),
        _ => format!("{} unread", n.active_count),
    };
    if n.has_critical {
        return vec![Affordance {
            id: "notifications.critical",
            kind: AffordanceKind::Warning,
            title: "Critical notification".into(),
            detail,
            relevance: 0.8,
            reason: "an active notification is critical urgency",
            source: Layer::Notifications,
            action: AffordanceAction::None,
        }];
    }
    vec![Affordance {
        id: "notifications.unread",
        kind: AffordanceKind::Info,
        title: match n.active_count {
            1 => "1 notification".into(),
            c => format!("{c} notifications"),
        },
        detail,
        // A little more present as they stack, but stays quiet ambient info.
        relevance: (0.3 + 0.05 * n.active_count.min(4) as f32).clamp(0.0, 0.5),
        reason: "unread notifications present",
        source: Layer::Notifications,
        action: AffordanceAction::None,
    }]
}

/// A copied URL or code snippet is strong intent — offer to act on it.
fn selection_provider(ctx: &ContextState) -> Vec<Affordance> {
    let s = &ctx.selection;
    if s.char_count == 0 {
        return vec![];
    }
    if s.contains_url {
        // **selection module** — a copied URL is intent to open it.
        // A real Control: xdg-open the link with the default handler.
        let url = s.highlighted_text.clone().unwrap_or_default();
        vec![Affordance {
            id: "selection.url",
            kind: AffordanceKind::Control,
            title: "Open copied link".into(),
            detail: url.clone(),
            relevance: 0.5,
            reason: "clipboard holds a url",
            source: Layer::Selection,
            action: AffordanceAction::OpenUrl(url),
        }]
    } else if s.is_path {
        // A copied absolute path is intent to open that file/folder in its
        // default app (xdg-open handles a plain path).
        let path = s.highlighted_text.clone().unwrap_or_default();
        let path = path.trim().to_string();
        vec![Affordance {
            id: "selection.open_path",
            kind: AffordanceKind::Control,
            title: "Open file".into(),
            detail: path.rsplit('/').next().unwrap_or(&path).to_string(),
            relevance: 0.5,
            reason: "clipboard holds a file path",
            source: Layer::Selection,
            action: AffordanceAction::OpenUrl(path),
        }]
    } else if let Some(text) = s
        .highlighted_text
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        // Any other copied text (prose or code) is a strong search intent — a
        // pasted error message, a term, a snippet. Offer to search the web for
        // it (xdg-open a query URL). The query is percent-encoded so spaces and
        // metacharacters travel intact. Covers "text/document editing" and
        // "web browsing" from the catalog with a single universal control.
        let query = url_encode_query(text.trim());
        vec![Affordance {
            id: "selection.search",
            kind: AffordanceKind::Control,
            title: "Search the web".into(),
            detail: text.chars().take(48).collect(),
            relevance: 0.42,
            reason: "clipboard holds searchable text",
            source: Layer::Selection,
            action: AffordanceAction::OpenUrl(format!("https://duckduckgo.com/?q={query}")),
        }]
    } else {
        vec![]
    }
}

/// Percent-encode `s` for use in a URL query value (RFC 3986 unreserved kept,
/// everything else `%XX`). Small and dependency-free — the only transform the
/// search control needs.
fn url_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        AppInternalContext, DeployHealth, GitContext, MediaState, SystemMetrics, TextSelection,
    };

    /// A context with everything's source layer marked alive, so freshness
    /// gating doesn't hide provider output under test.
    fn live_ctx() -> ContextState {
        let mut ctx = ContextState::default();
        ctx.health.compositor.alive = true;
        ctx.health.hardware.alive = true;
        ctx.health.behavior.alive = true;
        ctx.health.app_bridge.alive = true;
        ctx.health.selection.alive = true;
        ctx.health.system.alive = true;
        ctx.health.notifications.alive = true;
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
        // A roomy cap so the ambient now_playing Info isn't crowded out of the
        // capped set by the media *controls* cluster (this test is about the
        // freshness gate, not the cap).
        let roomy = Tuning {
            max_items: 12,
            ..Default::default()
        };
        // With hardware alive it shows…
        assert!(decide(&ctx, &roomy)
            .items
            .iter()
            .any(|a| a.id == "media.now_playing"));
        // …but if the hardware layer is dead, it must not.
        ctx.health.hardware.alive = false;
        assert!(decide(&ctx, &roomy)
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
    fn long_coding_session_suggests_a_break() {
        let ctx = {
            let mut c = live_ctx();
            c.window.class = "Code".into();
            c.window.pid = 1;
            c
        };
        // No time elapsed → nothing; over an hour → the nudge appears.
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| a.id != "session.long_coding"));
        let temporal = Temporal {
            activity_secs: 3700,
            ..Default::default()
        };
        assert!(decide_with(&ctx, &temporal, &Tuning::default())
            .items
            .iter()
            .any(|a| a.id == "session.long_coding"));
    }

    #[test]
    fn failure_streak_escalates_to_a_hand() {
        let ctx = live_ctx();
        let temporal = Temporal {
            failure_streak: 3,
            ..Default::default()
        };
        let a = decide_with(&ctx, &temporal, &Tuning::default())
            .items
            .into_iter()
            .find(|a| a.id == "session.failure_streak")
            .expect("streak should surface");
        assert_eq!(a.kind, AffordanceKind::Action);
    }

    #[test]
    fn critical_notification_is_a_warning_and_needs_a_live_daemon() {
        let mut ctx = live_ctx();
        ctx.notifications = crate::state::NotificationContext {
            active_count: 2,
            has_critical: true,
            latest_app: "Alarm".into(),
            latest_summary: "Wake up".into(),
        };
        let a = decide(&ctx, &Tuning::default())
            .items
            .into_iter()
            .find(|a| a.id == "notifications.critical")
            .expect("critical notification should surface");
        assert_eq!(a.kind, AffordanceKind::Warning);
        assert!(a.detail.contains("Alarm"));
        // If the notification daemon is unreachable, its state must not surface.
        ctx.health.notifications.alive = false;
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| !a.id.starts_with("notifications.")));
    }

    #[test]
    fn plain_unread_is_ambient_info_that_fades_for_experts() {
        let mut ctx = live_ctx();
        ctx.notifications = crate::state::NotificationContext {
            active_count: 1,
            has_critical: false,
            latest_app: "Mail".into(),
            latest_summary: "Hello".into(),
        };
        let novice = |skill| {
            decide(
                &ctx,
                &Tuning {
                    skill,
                    ..Default::default()
                },
            )
            .items
            .iter()
            .find(|a| a.id == "notifications.unread")
            .map(|a| a.relevance)
        };
        let expert_rel = novice(1.0).expect("unread should surface");
        let novice_rel = novice(0.0).expect("unread should surface");
        assert!(expert_rel < novice_rel, "ambient info fades for experts");
    }

    #[test]
    fn copied_url_offers_to_open() {
        let mut ctx = live_ctx();
        ctx.selection = TextSelection {
            highlighted_text: Some("https://example.com".into()),
            char_count: 19,
            is_code: false,
            contains_url: true,
            is_path: false,
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
    fn failed_activation_outranks_and_hides_the_stale_generation() {
        let mut ctx = live_ctx();
        // A failed switch implies both flags; we surface only the root cause.
        ctx.deploy = DeployHealth {
            not_activated: true,
            stale_generation: true,
        };
        let opts = decide(&ctx, &Tuning::default());
        let a = opts
            .items
            .iter()
            .find(|a| a.id == "deploy.not_activated")
            .expect("failed activation should surface");
        assert_eq!(a.kind, AffordanceKind::Warning);
        assert!(opts.items.iter().all(|a| a.id != "deploy.stale_generation"));
    }

    #[test]
    fn stale_generation_surfaces_and_needs_a_live_sensor() {
        let mut ctx = live_ctx();
        ctx.deploy = DeployHealth {
            not_activated: false,
            stale_generation: true,
        };
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .any(|a| a.id == "deploy.stale_generation"));
        // If the deploy sensor is dead, its warning vanishes (freshness gate) —
        // never assert stale drift from a source we can't see.
        ctx.health.system.alive = false;
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| !a.id.starts_with("deploy.")));
    }

    #[test]
    fn calibrate_scales_scaffolding_by_skill_but_never_safety() {
        // Safety is untouchable regardless of skill.
        assert_eq!(calibrate(AffordanceKind::Warning, 0.9, 1.0), 0.9);
        assert_eq!(calibrate(AffordanceKind::Warning, 0.9, 0.0), 0.9);
        // An Action fades for the expert, full for the novice.
        assert!(
            calibrate(AffordanceKind::Action, 0.4, 1.0)
                < calibrate(AffordanceKind::Action, 0.4, 0.0)
        );
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
        assert!(opts
            .items
            .iter()
            .any(|a| a.id == "compositor.screencasting"));
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
            decide(
                &ctx,
                &Tuning {
                    skill,
                    // Roomy cap so the ambient Info isn't crowded out of the
                    // capped set by the media controls cluster.
                    max_items: 12,
                    ..Default::default()
                },
            )
            .items
            .iter()
            .find(|a| a.id == "media.now_playing")
            .map(|a| a.relevance)
            .unwrap()
        };
        assert!(rel(1.0) < rel(0.0));
    }

    // --- action modules -----------------------------------------------------

    fn find<'a>(opts: &'a OptionSet, id: &str) -> Option<&'a Affordance> {
        opts.items.iter().find(|a| a.id == id)
    }

    #[test]
    fn media_controls_are_actionable_and_never_fade() {
        let mut ctx = live_ctx();
        ctx.media = Some(MediaState {
            player_name: "mpv".into(),
            title: "clip".into(),
            artist: String::new(),
            is_playing: true,
        });
        ctx.metrics.has_backlight = true; // a laptop panel → brightness offered
        let roomy = Tuning {
            max_items: 12,
            skill: 1.0, // an expert — controls must NOT fade
            ..Default::default()
        };
        let opts = decide(&ctx, &roomy);
        let pp = find(&opts, "media.playpause").expect("play/pause control");
        assert_eq!(pp.kind, AffordanceKind::Control);
        assert_eq!(pp.title, "Pause"); // playing → offers Pause
        assert!(pp.action.is_actionable());
        assert!(matches!(&pp.action, AffordanceAction::Spawn { argv }
            if argv == &["playerctl", "play-pause"]));
        // Volume + (with a backlight) brightness controls present.
        assert!(find(&opts, "media.vol_up").is_some());
        assert!(find(&opts, "media.bright_down").is_some());
        // A Control is not faded even for a max-skill expert.
        assert!(pp.relevance >= 0.6);

        // Without a backlight (a desktop / VM) brightness is not offered, but
        // the media transport still is.
        ctx.metrics.has_backlight = false;
        let no_bl = decide(&ctx, &roomy);
        assert!(find(&no_bl, "media.bright_down").is_none());
        assert!(find(&no_bl, "media.playpause").is_some());
        assert!(find(&no_bl, "media.vol_up").is_some());

        // Paused → the toggle offers Play instead.
        if let Some(m) = ctx.media.as_mut() {
            m.is_playing = false;
        }
        assert_eq!(
            find(&decide(&ctx, &roomy), "media.playpause")
                .unwrap()
                .title,
            "Play"
        );
    }

    #[test]
    fn git_commit_and_push_only_while_coding() {
        let mut ctx = live_ctx();
        ctx.git = GitContext {
            repo_root: Some("/home/max/proj".into()),
            branch: Some("main".into()),
            is_dirty: true,
        };
        // A terminal in a repo → Coding: commit & push surface as controls.
        ctx.window.class = "foot".into();
        ctx.window.pid = 1;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let commit = find(&opts, "git.commit").expect("commit control while coding");
        assert_eq!(commit.kind, AffordanceKind::Control);
        assert!(matches!(&commit.action, AffordanceAction::Spawn { argv }
            if argv[0] == "git" && argv.contains(&"/home/max/proj".to_string())
               && argv.contains(&"commit".to_string())));
        assert!(find(&opts, "git.push").is_some());

        // A browser focused in the same repo dir → Browsing, NOT coding: the
        // git controls clear away (only the branch/dirty info would remain).
        ctx.window.class = "firefox".into();
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(find(&opts, "git.commit").is_none());
        assert!(find(&opts, "git.push").is_none());
    }

    #[test]
    fn live_mic_offers_a_mute_control() {
        let mut ctx = live_ctx();
        ctx.audio.is_mic_active = true;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let mute = find(&opts, "audio.mic_mute").expect("mute control when mic live");
        assert_eq!(mute.kind, AffordanceKind::Control);
        assert!(matches!(&mute.action, AffordanceAction::Spawn { argv }
            if argv.contains(&"set-mute".to_string())));
        // The warning is still there too.
        assert!(find(&opts, "audio.mic_live").is_some());
    }

    #[test]
    fn copied_url_action_opens_it() {
        let mut ctx = live_ctx();
        ctx.selection = TextSelection {
            highlighted_text: Some("https://example.com".into()),
            char_count: 19,
            is_code: false,
            contains_url: true,
            is_path: false,
        };
        let opts = decide(&ctx, &Tuning::default());
        let url = find(&opts, "selection.url").expect("url control");
        assert_eq!(url.kind, AffordanceKind::Control);
        assert_eq!(
            url.action,
            AffordanceAction::OpenUrl("https://example.com".into())
        );
    }

    #[test]
    fn media_offers_seek_controls() {
        let mut ctx = live_ctx();
        ctx.media = Some(MediaState {
            player_name: "vlc".into(),
            title: "clip".into(),
            artist: String::new(),
            is_playing: true,
        });
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let back = find(&opts, "media.seek_back").expect("seek-back control");
        assert!(matches!(&back.action, AffordanceAction::Spawn { argv }
            if argv == &["playerctl", "position", "10-"]));
        let fwd = find(&opts, "media.seek_fwd").expect("seek-fwd control");
        assert!(matches!(&fwd.action, AffordanceAction::Spawn { argv }
            if argv == &["playerctl", "position", "10+"]));
        // Seek outranks track-skip so it leads the cluster (video-first).
        let next = find(&opts, "media.next").unwrap();
        assert!(back.relevance > next.relevance);
    }

    #[test]
    fn copied_text_offers_a_web_search_url_encoded() {
        let mut ctx = live_ctx();
        ctx.selection = TextSelection {
            highlighted_text: Some("rust borrow checker error E0502".into()),
            char_count: 31,
            is_code: false,
            contains_url: false,
            is_path: false,
        };
        let opts = decide(&ctx, &Tuning::default());
        let s = find(&opts, "selection.search").expect("search control");
        assert_eq!(s.kind, AffordanceKind::Control);
        assert_eq!(
            s.action,
            AffordanceAction::OpenUrl(
                "https://duckduckgo.com/?q=rust%20borrow%20checker%20error%20E0502".into()
            )
        );
        // Whitespace-only selection offers nothing.
        ctx.selection.highlighted_text = Some("   ".into());
        assert!(find(&decide(&ctx, &Tuning::default()), "selection.search").is_none());
    }

    #[test]
    fn copied_path_offers_open_file() {
        let mut ctx = live_ctx();
        ctx.selection = TextSelection {
            highlighted_text: Some("/home/max/notes/todo.md".into()),
            char_count: 23,
            is_code: false,
            contains_url: false,
            is_path: true,
        };
        let opts = decide(&ctx, &Tuning::default());
        let o = find(&opts, "selection.open_path").expect("open-file control");
        assert_eq!(o.kind, AffordanceKind::Control);
        assert_eq!(o.detail, "todo.md"); // basename shown
        assert_eq!(
            o.action,
            AffordanceAction::OpenUrl("/home/max/notes/todo.md".into())
        );
        // A path takes precedence over the generic web-search fallback.
        assert!(find(&opts, "selection.search").is_none());
    }

    #[test]
    fn url_encoding_escapes_reserved_and_keeps_unreserved() {
        assert_eq!(url_encode_query("a b&c"), "a%20b%26c");
        assert_eq!(url_encode_query("A-z_0.9~"), "A-z_0.9~");
        assert_eq!(url_encode_query("$(x)"), "%24%28x%29");
    }

    #[test]
    fn busy_context_blends_and_caps_by_relevance() {
        // Coding in a dirty repo WITH media playing: both modules' controls are
        // eligible. The mind ranks by relevance and caps — the surface then
        // sees a blended, de-cluttered set, never everything at once.
        let mut ctx = live_ctx();
        ctx.window.class = "foot".into();
        ctx.window.pid = 1;
        ctx.git = GitContext {
            repo_root: Some("/home/max/p".into()),
            branch: Some("main".into()),
            is_dirty: true,
        };
        ctx.media = Some(MediaState {
            player_name: "vlc".into(),
            title: "t".into(),
            artist: String::new(),
            is_playing: true,
        });
        // Cap of 4: only the four highest-relevance controls survive.
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 4,
                ..Default::default()
            },
        );
        assert_eq!(opts.items.len(), 4);
        // Descending, and all above the min-relevance floor.
        for w in opts.items.windows(2) {
            assert!(w[0].relevance >= w[1].relevance);
        }
        assert!(opts.items.iter().all(|a| a.relevance >= 0.2));
        // The top control is the highest-scoring one present (media play/pause
        // at 0.66 beats git.commit at 0.64).
        assert_eq!(opts.items[0].id, "media.playpause");
        // Both modules are represented in the blended top set.
        assert!(opts.items.iter().any(|a| a.id.starts_with("media.")));
        assert!(opts.items.iter().any(|a| a.id.starts_with("git.")));
    }

    #[test]
    fn high_cpu_offers_a_monitor_control() {
        let mut ctx = live_ctx();
        ctx.metrics.cpu_usage_pct = 92.0;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let c = find(&opts, "system.high_cpu").expect("high-cpu control");
        assert_eq!(c.kind, AffordanceKind::Control);
        assert!(c.action.is_actionable());
        // Below the threshold it does not surface.
        ctx.metrics.cpu_usage_pct = 40.0;
        assert!(find(&decide(&ctx, &Tuning::default()), "system.high_cpu").is_none());
    }
}

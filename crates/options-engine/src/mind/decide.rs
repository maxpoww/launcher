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
    memory_provider,
    media_provider,
    media_controls_provider,
    git_provider,
    coding_tools_provider,
    rerun_provider,
    files_here_provider,
    downloads_provider,
    focus_churn_provider,
    shell_error_provider,
    diagnostics_provider,
    install_missing_provider,
    editor_provider,
    selection_provider,
    mic_provider,
    camera_provider,
    fullscreen_provider,
    browser_provider,
    reading_provider,
    presentation_provider,
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
    if matches!(
        id,
        "git.commit"
            | "git.push"
            | "git.pull"
            | "git.open_remote"
            | "coding.terminal_here"
            | "editor.open_folder"
            | "editor.run"
    ) {
        return activity == Activity::Coding;
    }
    // "Open files here" / "Re-run last" / "Install <missing>" fit terminal work
    // (and coding) — a stale shell exit shouldn't fire them while browsing.
    if matches!(id, "files.open_here" | "shell.rerun" | "shell.install_missing") {
        return matches!(activity, Activity::Terminal | Activity::Coding);
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

/// Situate an affordance's relevance to the current activity. The Mind ranks a
/// blended set (media + git + …), and a track playing quietly *behind* your
/// work shouldn't outrank the controls for the work itself. So media controls
/// are damped when media is background — i.e. the activity is something more
/// purposeful (Coding, Communication, Browsing, Reading, Terminal). When media
/// IS the activity (watching), they keep full weight.
fn contextual_relevance(id: &str, base: f32, activity: Activity, media_fg: bool) -> f32 {
    let media_is_background = id.starts_with("media.")
        && !media_fg
        && !matches!(
            activity,
            Activity::Media | Activity::Idle | Activity::Unknown
        );
    if media_is_background {
        base * 0.65
    } else {
        base
    }
}

/// Whether media is the FOREGROUND thing — full-weight controls. True for the
/// Media activity, and for a **browser video tab**: a browser is focused AND
/// the active MPRIS player is that browser (Chrome/Firefox expose MPRIS when a
/// tab plays video). Music playing in another window while you browse is NOT
/// foreground (its player isn't the browser), so it stays damped.
fn media_is_foreground(ctx: &ContextState, activity: Activity) -> bool {
    if activity == Activity::Media {
        return true;
    }
    if activity == Activity::Browsing {
        if let Some(m) = ctx.media.as_ref().filter(|m| m.is_playing) {
            let p = m.player_name.to_lowercase();
            return BROWSER_PLAYERS.iter().any(|b| p.contains(b));
        }
    }
    false
}

/// MPRIS player-name fragments that identify a browser (a video tab).
const BROWSER_PLAYERS: &[&str] = &[
    "firefox",
    "mozilla",
    "chrom",
    "chrome",
    "brave",
    "vivaldi",
    "edge",
    "librewolf",
    "zen",
];

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
        let mut out = vec![Affordance {
            id: "system.battery_critical",
            kind: AffordanceKind::Warning,
            title: format!("Battery low — {pct}%"),
            detail: "Plug in soon".into(),
            relevance: 0.97,
            reason: "battery <=15% on power",
            source: Layer::Hardware,
            action: AffordanceAction::None,
        }];
        // The panel backlight is usually the single biggest draw, so on a
        // laptop turn the warning into something actionable: one tap dims the
        // screen to a still-usable 40% to stretch the remaining runtime. Gated
        // on a real backlight (never on a desktop/VM). Ranks just under the
        // warning so it reads as its remedy.
        if ctx.metrics.has_backlight {
            out.push(Affordance {
                id: "system.battery_dim",
                kind: AffordanceKind::Control,
                title: "Dim screen".into(),
                detail: "Save power".into(),
                relevance: 0.95,
                reason: "cut the biggest draw to extend runtime",
                source: Layer::Hardware,
                action: spawn(&["brightnessctl", "set", "40%"]),
            });
        }
        out
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

/// Sustained memory pressure is worth the same "show me what's using it" offer
/// as high CPU. Gated above the CPU pill's threshold so at most one monitor
/// control shows: when both are high the CPU one already covers it, so this
/// only fires when memory is the distinct problem.
fn memory_provider(ctx: &ContextState) -> Vec<Affordance> {
    if ctx.metrics.ram_usage_pct < 90.0 || ctx.metrics.cpu_usage_pct >= 85.0 {
        return vec![];
    }
    vec![Affordance {
        id: "system.high_mem",
        kind: AffordanceKind::Control,
        title: "High memory".into(),
        detail: format!("{:.0}% — open monitor", ctx.metrics.ram_usage_pct),
        relevance: 0.42,
        reason: "ram >=90%",
        source: Layer::Hardware,
        action: spawn(&["foot", "btop"]),
    }]
}

/// Build a fire-and-forget [`AffordanceAction::Spawn`] from a static argv.
/// Single-quote a value for embedding inside an inner `sh -c` string (the few
/// controls that need a pipeline). Engine-owned paths only, but quoted anyway
/// so a space or metacharacter in a repo path can never break the command.
fn shell_arg(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

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
            id: "media.mute",
            kind: AffordanceKind::Control,
            title: "Mute".into(),
            detail: String::new(),
            relevance: 0.605,
            reason: "mute/unmute the audio",
            source: Layer::Hardware,
            action: spawn(&["wpctl", "set-mute", SINK, "toggle"]),
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
            out.push(Affordance {
                id: "git.diff",
                kind: AffordanceKind::Control,
                title: "Show diff".into(),
                detail: format!("on {branch}"),
                relevance: 0.55,
                reason: "review the uncommitted changes",
                source: Layer::Hardware,
                // A terminal pager over the diff; `-C <root>` and the fixed
                // pipeline are engine-constructed (no user text) → shell-safe.
                action: spawn(&[
                    "foot",
                    "sh",
                    "-c",
                    &format!("git -C {} diff | less -R", shell_arg(root)),
                ]),
            });
        }
    }
    // Pull is useful whether or not the tree is dirty (grab upstream before
    // you start). --ff-only never creates a merge and aborts cleanly on
    // divergence or a dirty conflict, so it is safe to offer unconditionally.
    if let Some(root) = ctx.git.repo_root.as_deref().and_then(|p| p.to_str()) {
        out.push(Affordance {
            id: "git.pull",
            kind: AffordanceKind::Control,
            title: "Pull".into(),
            detail: format!("on {branch}"),
            relevance: 0.58,
            reason: "fast-forward to the remote",
            source: Layer::Hardware,
            action: spawn(&["git", "-C", root, "pull", "--ff-only"]),
        });
    }
    // Open the repo's remote (GitHub/GitLab/…) in the browser.
    if let Some(web) = &ctx.git.remote_url {
        out.push(Affordance {
            id: "git.open_remote",
            kind: AffordanceKind::Control,
            title: "Open remote".into(),
            detail: web
                .strip_prefix("https://")
                .unwrap_or(web)
                .trim_start_matches("www.")
                .to_string(),
            relevance: 0.5,
            reason: "repo has a browsable remote",
            source: Layer::Hardware,
            action: AffordanceAction::OpenUrl(web.clone()),
        });
    }
    out
}

/// **coding-tools module** — a terminal opened right in the repo you're working
/// in. Most useful from a GUI editor (no shell to hand); reuses the sensed
/// `git.repo_root`. Gated to Coding via `fits_activity`.
fn coding_tools_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(root) = ctx.git.repo_root.as_deref().and_then(|p| p.to_str()) else {
        return vec![];
    };
    vec![Affordance {
        id: "coding.terminal_here",
        kind: AffordanceKind::Control,
        title: "Terminal here".into(),
        detail: root.rsplit('/').next().unwrap_or(root).to_string(),
        relevance: 0.45,
        reason: "open a terminal in the repo",
        source: Layer::Hardware,
        action: spawn(&["foot", "--working-directory", root]),
    }]
}

/// **dev-run module** — re-run the last shell command in a fresh terminal at
/// its folder (both from the shell bridge). Useful after editing: click to
/// re-run the last build/test without hunting for the terminal. The command is
/// the user's own; it rides one argv slot into `zsh -ic`, shell-quoted for the
/// outer shell, so it runs verbatim with no extra injection surface.
fn rerun_provider(ctx: &ContextState) -> Vec<Affordance> {
    let ai = &ctx.app_internal;
    let (Some(cmd), Some(cwd)) = (
        ai.shell_last_cmd
            .as_deref()
            .filter(|c| !c.trim().is_empty()),
        ai.shell_cwd.as_deref().and_then(|p| p.to_str()),
    ) else {
        return vec![];
    };
    vec![Affordance {
        id: "shell.rerun",
        kind: AffordanceKind::Control,
        title: "Re-run last".into(),
        detail: cmd.chars().take(40).collect(),
        relevance: 0.47,
        reason: "re-run the last command in a fresh terminal",
        source: Layer::AppBridge,
        action: AffordanceAction::Spawn {
            argv: vec![
                "foot".into(),
                "--working-directory".into(),
                cwd.to_string(),
                "zsh".into(),
                "-ic".into(),
                format!("{cmd}; exec zsh"),
            ],
        },
    }]
}

/// **files module** — open the shell's current folder in the file manager. Fed
/// by the shell bridge's cwd; xdg-open on a directory launches the default file
/// manager there. Gated to terminal/coding activity in `fits_activity`.
fn files_here_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(cwd) = ctx
        .app_internal
        .shell_cwd
        .as_deref()
        .and_then(|p| p.to_str())
    else {
        return vec![];
    };
    vec![Affordance {
        id: "files.open_here",
        kind: AffordanceKind::Control,
        title: "Open files here".into(),
        detail: cwd.rsplit('/').next().unwrap_or(cwd).to_string(),
        relevance: 0.46,
        reason: "open the shell's folder in the file manager",
        source: Layer::AppBridge,
        action: AffordanceAction::OpenUrl(cwd.to_string()),
    }]
}

/// A file just landed in Downloads — offer to open it with its default app.
/// Transient (the collector clears it after a short window), so this is the
/// "you just downloaded X" moment, not a permanent pin. xdg-open picks the
/// right handler by type (PDF viewer, archive manager, image viewer, …).
fn downloads_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(path) = ctx.recent_download.as_deref().and_then(|p| p.to_str()) else {
        return vec![];
    };
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut out = vec![Affordance {
        id: "downloads.open",
        kind: AffordanceKind::Control,
        title: "Open download".into(),
        detail: name.to_string(),
        relevance: 0.62,
        reason: "a file just finished downloading",
        source: Layer::Hardware,
        action: AffordanceAction::OpenUrl(path.to_string()),
    }];
    // An archive is more likely wanted extracted than merely opened to browse
    // its contents, so offer a direct one-tap "Extract here" alongside (it
    // unpacks next to the file via the archive manager). Ranked just above
    // plain open for archives.
    if is_archive(name) {
        out.push(Affordance {
            id: "downloads.extract",
            kind: AffordanceKind::Control,
            title: "Extract here".into(),
            detail: name.to_string(),
            relevance: 0.64,
            reason: "the download is an archive",
            source: Layer::Hardware,
            action: spawn(&["file-roller", "--extract-here", path]),
        });
    }
    out
}

/// Whether a filename looks like a compressed archive the archive manager can
/// unpack. Matches the common single- and compound-suffix families by their
/// trailing tokens, case-insensitively.
fn is_archive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    const EXACT: &[&str] = &[
        ".zip", ".tar", ".tgz", ".txz", ".tbz", ".tbz2", ".gz", ".xz", ".bz2", ".zst", ".7z",
        ".rar", ".lz", ".lzma", ".cab",
    ];
    const COMPOUND: &[&str] = &[".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tar.lz"];
    COMPOUND.iter().any(|s| lower.ends_with(s)) || EXACT.iter().any(|s| lower.ends_with(s))
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
            let cmd = ctx.app_internal.shell_last_cmd.as_deref();
            let detail = cmd
                .map(|c| format!("{c} → exit {code}"))
                .unwrap_or_else(|| format!("exit {code}"));
            // A failed command: offer to look it up. "Turn frustration into
            // revelation" — a web search for the command that just failed. A
            // Control (the direct thing you'd reach for), fed by the shell
            // bridge. Without a captured command there's nothing to search, so
            // it stays a plain info line.
            match cmd.filter(|c| !c.trim().is_empty()) {
                Some(c) => vec![Affordance {
                    id: "shell.search_error",
                    kind: AffordanceKind::Control,
                    title: "Search the error".into(),
                    detail,
                    relevance: 0.55,
                    reason: "nonzero shell exit code with a known command",
                    source: Layer::AppBridge,
                    action: AffordanceAction::OpenUrl(format!(
                        "https://duckduckgo.com/?q={}",
                        url_encode_query(c.trim())
                    )),
                }],
                None => vec![Affordance {
                    id: "shell.last_failed",
                    kind: AffordanceKind::Info,
                    title: "Last command failed".into(),
                    detail,
                    relevance: 0.4,
                    reason: "nonzero shell exit code",
                    source: Layer::AppBridge,
                    action: AffordanceAction::None,
                }],
            }
        }
        _ => vec![],
    }
}

/// The command name from a shell line: its first whitespace token, skipping a
/// leading `sudo` and `VAR=val` assignments. `None` when there's nothing usable.
fn first_command_token(line: &str) -> Option<&str> {
    for tok in line.split_whitespace() {
        if tok == "sudo" || (tok.contains('=') && !tok.starts_with('=')) {
            continue; // env-assignment or a sudo prefix, not the command
        }
        // Only a bare program name is installable — skip paths and pipelines.
        if tok.contains('/') || tok.starts_with('-') {
            return None;
        }
        return Some(tok);
    }
    None
}

/// A `command not found` (exit 127) on NixOS is a golden moment: the tool you
/// wanted has a package. Offer to search nixpkgs for it — one tap opens the
/// launcher's Install search pre-filled with the missing command. Distinct from
/// "Search the error" (a web lookup); this leads straight to installing it.
fn install_missing_provider(ctx: &ContextState) -> Vec<Affordance> {
    if ctx.app_internal.shell_exit_code != Some(127) {
        return vec![];
    }
    let Some(cmd) = ctx
        .app_internal
        .shell_last_cmd
        .as_deref()
        .and_then(first_command_token)
    else {
        return vec![];
    };
    vec![Affordance {
        id: "shell.install_missing",
        kind: AffordanceKind::Control,
        title: format!("Install {cmd}?"),
        detail: "not found — search nixpkgs".into(),
        relevance: 0.6,
        reason: "command-not-found (exit 127) with a bare program name",
        source: Layer::AppBridge,
        action: AffordanceAction::Daemon(format!("pkgsearch:{cmd}")),
    }]
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

/// **editor module** — the file you're editing (fed by the editor bridge):
/// offer to open its folder in the file manager. Its own control so it works
/// even in a repo where the git controls already show.
fn editor_provider(ctx: &ContextState) -> Vec<Affordance> {
    let Some(dir) = ctx
        .app_internal
        .editor_file
        .as_deref()
        .and_then(|f| f.parent())
        .and_then(|d| d.to_str())
    else {
        return vec![];
    };
    let mut out = vec![Affordance {
        id: "editor.open_folder",
        kind: AffordanceKind::Control,
        title: "Open folder".into(),
        detail: dir.rsplit('/').next().unwrap_or(dir).to_string(),
        relevance: 0.43,
        reason: "open the edited file's folder",
        source: Layer::AppBridge,
        action: AffordanceAction::OpenUrl(dir.to_string()),
    }];
    // Language-appropriate "Run this file" for scripting languages: run the
    // interpreter on the edited file in a fresh terminal (the file path is
    // quoted for the inner shell; both come from the editor bridge, not raw
    // user text). Compiled languages are project-not-file builds — skipped.
    if let (Some(file), Some(runner)) = (
        ctx.app_internal
            .editor_file
            .as_deref()
            .and_then(|f| f.to_str()),
        ctx.app_internal
            .editor_language
            .as_deref()
            .and_then(runner_for_language),
    ) {
        out.push(Affordance {
            id: "editor.run",
            kind: AffordanceKind::Control,
            title: "Run this file".into(),
            detail: runner.into(),
            relevance: 0.5,
            reason: "run the edited script",
            source: Layer::AppBridge,
            action: AffordanceAction::Spawn {
                argv: vec![
                    "foot".into(),
                    "--working-directory".into(),
                    dir.to_string(),
                    "sh".into(),
                    "-c".into(),
                    format!("{runner} {}; exec zsh", shell_arg(file)),
                ],
            },
        });
    }
    out
}

/// The interpreter for a scripting language's filetype, or `None` for compiled
/// / unknown languages (a per-project build, not a single-file run).
fn runner_for_language(lang: &str) -> Option<&'static str> {
    match lang.to_lowercase().as_str() {
        "python" => Some("python3"),
        "ruby" => Some("ruby"),
        "javascript" => Some("node"),
        "sh" | "bash" => Some("bash"),
        "lua" => Some("lua"),
        "perl" => Some("perl"),
        "php" => Some("php"),
        _ => None,
    }
}

/// A live microphone — awareness that you're being heard (and the seat of the
/// Communication activity).
/// **browser module** — a browser is focused: offer "Find in page" (Ctrl+F).
/// The action is a compositor keystroke (no bridge, no extra dependency); the
/// same mechanism the clipboard paste uses. Ctrl+F is universal across
/// browsers, so this works from the window class alone.
fn browser_provider(ctx: &ContextState) -> Vec<Affordance> {
    if infer_activity(ctx) != Activity::Browsing {
        return vec![];
    }
    vec![
        Affordance {
            id: "browser.find",
            kind: AffordanceKind::Control,
            title: "Find in page".into(),
            detail: String::new(),
            relevance: 0.44,
            reason: "find on the current page",
            source: Layer::Compositor,
            action: AffordanceAction::Daemon("find_in_page".into()),
        },
        // Reopen-closed-tab (Ctrl+Shift+T) is universal across every browser and
        // the classic "oops" recovery. A compositor keystroke, same path as
        // Find; ranked just under it (Find is the more frequent reach).
        Affordance {
            id: "browser.reopen_tab",
            kind: AffordanceKind::Control,
            title: "Reopen closed tab".into(),
            detail: String::new(),
            relevance: 0.43,
            reason: "restore the last-closed tab",
            source: Layer::Compositor,
            action: AffordanceAction::Daemon("reopen_tab".into()),
        },
    ]
}

/// Window classes we treat as document/PDF readers (substring, lower-cased).
const READERS: &[&str] = &[
    "papers",
    "evince",
    "zathura",
    "okular",
    "mupdf",
    "xpdf",
    "org.gnome.documents",
    "foliate",
    "sioyek",
];

/// Office/presentation window classes (substring, lower-cased).
const PRESENTERS: &[&str] = &[
    "impress",
    "libreoffice",
    "soffice",
    "powerpoint",
    "onlyoffice",
];

/// **presentation module** — an office/slides app. Editing the deck (windowed):
/// offer "Present" (F5, start the slideshow). Presenting (fullscreen): offer
/// Next / Previous slide (arrow keys). All via the compositor keystroke.
fn presentation_provider(ctx: &ContextState) -> Vec<Affordance> {
    let class = ctx.window.class.to_lowercase();
    if !PRESENTERS.iter().any(|p| class.contains(p)) {
        return vec![];
    }
    // Not yet presenting: one tap starts the slideshow from the first slide.
    if !ctx.window.is_fullscreen {
        return vec![Affordance {
            id: "slides.present",
            kind: AffordanceKind::Control,
            title: "Present".into(),
            detail: "Start the slideshow".into(),
            relevance: 0.5,
            reason: "a presentation app is focused but not presenting",
            source: Layer::Compositor,
            action: AffordanceAction::Daemon("present".into()),
        }];
    }
    vec![
        Affordance {
            id: "slides.next",
            kind: AffordanceKind::Control,
            title: "Next slide".into(),
            detail: String::new(),
            relevance: 0.62,
            reason: "advance the slideshow",
            source: Layer::Compositor,
            action: AffordanceAction::Daemon("slide_next".into()),
        },
        Affordance {
            id: "slides.prev",
            kind: AffordanceKind::Control,
            title: "Previous slide".into(),
            detail: String::new(),
            relevance: 0.6,
            reason: "back a slide",
            source: Layer::Compositor,
            action: AffordanceAction::Daemon("slide_prev".into()),
        },
    ]
}

/// **reading module** — a PDF / document reader is focused: offer Find (Ctrl+F,
/// via the compositor keystroke) and, on a laptop, reading brightness. Keeps
/// the eyes comfortable and lets you jump to a term without leaving the mouse.
fn reading_provider(ctx: &ContextState) -> Vec<Affordance> {
    let class = ctx.window.class.to_lowercase();
    if ctx.window.pid == 0 || !READERS.iter().any(|r| class.contains(r)) {
        return vec![];
    }
    let mut out = vec![Affordance {
        id: "reading.find",
        kind: AffordanceKind::Control,
        title: "Find".into(),
        detail: String::new(),
        relevance: 0.5,
        reason: "find in the document",
        source: Layer::Compositor,
        action: AffordanceAction::Daemon("find_in_page".into()),
    }];
    if ctx.metrics.has_backlight {
        out.push(Affordance {
            id: "reading.bright_down",
            kind: AffordanceKind::Control,
            title: "Brightness −".into(),
            detail: String::new(),
            relevance: 0.46,
            reason: "dim for comfortable reading",
            source: Layer::Hardware,
            action: spawn(&["brightnessctl", "set", "10%-"]),
        });
        out.push(Affordance {
            id: "reading.bright_up",
            kind: AffordanceKind::Control,
            title: "Brightness +".into(),
            detail: String::new(),
            relevance: 0.45,
            reason: "brighten for reading",
            source: Layer::Hardware,
            action: spawn(&["brightnessctl", "set", "10%+"]),
        });
    }
    out
}

/// A fullscreen window is immersion — a video, a game, a presentation. Offer
/// Do-Not-Disturb so a notification doesn't pop over it. (The mic-live call
/// path offers DND too; this is the fullscreen path.)
fn fullscreen_provider(ctx: &ContextState) -> Vec<Affordance> {
    if !ctx.window.is_fullscreen {
        return vec![];
    }
    vec![
        Affordance {
            id: "window.fullscreen_dnd",
            kind: AffordanceKind::Control,
            title: "Do not disturb".into(),
            detail: "Fullscreen".into(),
            relevance: 0.5,
            reason: "silence notifications while fullscreen",
            source: Layer::Compositor,
            action: AffordanceAction::Daemon("toggle_dnd".into()),
        },
        // Screenshotting a game / video / slide is a common fullscreen reach.
        // grim writes a timestamped PNG to ~/Pictures (created if needed); the
        // whole command is engine-constructed, so the shell has no user text.
        Affordance {
            id: "window.screenshot",
            kind: AffordanceKind::Control,
            title: "Screenshot".into(),
            detail: "→ Pictures".into(),
            relevance: 0.47,
            reason: "capture the fullscreen content",
            source: Layer::Compositor,
            action: AffordanceAction::Spawn {
                argv: vec![
                    "sh".into(),
                    "-c".into(),
                    "mkdir -p ~/Pictures && grim ~/Pictures/screenshot-$(date +%Y%m%d-%H%M%S).png"
                        .into(),
                ],
            },
        },
    ]
}

/// A live camera — privacy awareness that you're on webcam (a video call).
/// A Warning like the live mic; there's no safe universal "camera off", so it
/// informs rather than acts.
fn camera_provider(ctx: &ContextState) -> Vec<Affordance> {
    if !ctx.metrics.is_camera_active {
        return vec![];
    }
    vec![Affordance {
        id: "camera.live",
        kind: AffordanceKind::Warning,
        title: "Camera is on".into(),
        detail: "You're on camera".into(),
        relevance: 0.82,
        reason: "a /dev/video device is open",
        source: Layer::Hardware,
        action: AffordanceAction::None,
    }]
}

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
        // In a call you don't want a notification popping over your screen
        // share — offer Do Not Disturb (mute the shell's notifications). An
        // internal daemon action.
        Affordance {
            id: "audio.call_dnd",
            kind: AffordanceKind::Control,
            title: "Do not disturb".into(),
            detail: String::new(),
            relevance: 0.6,
            reason: "silence notifications during a call",
            source: Layer::Hardware,
            action: AffordanceAction::Daemon("toggle_dnd".into()),
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
    } else if let Some(addr) = s
        .highlighted_text
        .as_deref()
        .map(str::trim)
        .filter(|t| looks_like_email(t))
    {
        // A copied email address is intent to write to it.
        vec![Affordance {
            id: "selection.email",
            kind: AffordanceKind::Control,
            title: "Compose email".into(),
            detail: addr.to_string(),
            relevance: 0.5,
            reason: "clipboard holds an email address",
            source: Layer::Selection,
            action: AffordanceAction::OpenUrl(format!("mailto:{addr}")),
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

/// Heuristic: the whole trimmed selection is a single email address
/// (`local@domain.tld`), no whitespace. Deliberately strict so a sentence that
/// merely mentions an address doesn't trigger it.
fn looks_like_email(t: &str) -> bool {
    let t = t.trim();
    if t.contains(char::is_whitespace) || t.len() > 254 {
        return false;
    }
    match t.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && domain
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        }
        None => false,
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
            position_secs: 0,
            length_secs: 0,
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
            position_secs: 0,
            length_secs: 0,
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
    fn critical_battery_offers_dim_only_on_a_laptop() {
        let mut ctx = live_ctx();
        // Critical battery on a machine with a backlight (a laptop panel).
        ctx.metrics = SystemMetrics {
            battery_pct: Some(8),
            is_charging: false,
            has_backlight: true,
            ..Default::default()
        };
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 8,
                ..Default::default()
            },
        );
        assert!(find(&opts, "system.battery_critical").is_some());
        let dim = find(&opts, "system.battery_dim").expect("dim control on a laptop");
        assert_eq!(dim.kind, AffordanceKind::Control);
        assert!(matches!(&dim.action, AffordanceAction::Spawn { argv }
            if argv[0] == "brightnessctl" && argv.contains(&"40%".to_string())));
        // Charging → no battery offers at all.
        ctx.metrics.is_charging = true;
        let charged = decide(&ctx, &Tuning::default());
        assert!(find(&charged, "system.battery_dim").is_none());
        assert!(find(&charged, "system.battery_critical").is_none());
        // A desktop (no backlight) gets the warning but no dim control.
        ctx.metrics.is_charging = false;
        ctx.metrics.has_backlight = false;
        let desktop = decide(
            &ctx,
            &Tuning {
                max_items: 8,
                ..Default::default()
            },
        );
        assert!(find(&desktop, "system.battery_critical").is_some());
        assert!(find(&desktop, "system.battery_dim").is_none());
    }

    #[test]
    fn command_not_found_offers_install_from_nixpkgs() {
        let mut ctx = live_ctx();
        ctx.window.class = "foot".into();
        ctx.window.pid = 1; // a terminal → Terminal activity
        ctx.app_internal = AppInternalContext {
            shell_last_cmd: Some("htop -d 5".into()),
            shell_exit_code: Some(127),
            ..Default::default()
        };
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let i = find(&opts, "shell.install_missing").expect("install-missing control");
        assert_eq!(i.title, "Install htop?");
        assert_eq!(i.action, AffordanceAction::Daemon("pkgsearch:htop".into()));
        // A normal failure (not 127) does not offer install.
        ctx.app_internal.shell_exit_code = Some(1);
        assert!(find(
            &decide(&ctx, &Tuning { max_items: 12, ..Default::default() }),
            "shell.install_missing"
        )
        .is_none());
        // 127 but the token is a path (./x) → not installable, no offer.
        ctx.app_internal.shell_exit_code = Some(127);
        ctx.app_internal.shell_last_cmd = Some("./build.sh".into());
        assert!(find(
            &decide(&ctx, &Tuning { max_items: 12, ..Default::default() }),
            "shell.install_missing"
        )
        .is_none());
        // Not while browsing (a stale exit code shouldn't fire it).
        ctx.app_internal.shell_last_cmd = Some("htop".into());
        ctx.window.class = "firefox".into();
        assert!(find(
            &decide(&ctx, &Tuning { max_items: 12, ..Default::default() }),
            "shell.install_missing"
        )
        .is_none());
    }

    #[test]
    fn first_command_token_skips_sudo_and_assignments() {
        assert_eq!(first_command_token("htop"), Some("htop"));
        assert_eq!(first_command_token("sudo nixos-rebuild"), Some("nixos-rebuild"));
        assert_eq!(first_command_token("FOO=1 BAR=2 ripgrep x"), Some("ripgrep"));
        assert_eq!(first_command_token("./local"), None);
        assert_eq!(first_command_token("/usr/bin/x"), None);
        assert_eq!(first_command_token(""), None);
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
        // With a captured command, the failure becomes an actionable
        // "Search the error" Control (web search for the command).
        let a = opts
            .items
            .iter()
            .find(|a| a.id == "shell.search_error")
            .expect("failure should surface");
        assert_eq!(a.kind, AffordanceKind::Control);
        assert!(a.detail.contains("cargo build"));
        assert_eq!(
            a.action,
            AffordanceAction::OpenUrl("https://duckduckgo.com/?q=cargo%20build".into())
        );
        // Without a captured command (no bridge) it's a plain info line.
        ctx.app_internal.shell_last_cmd = None;
        let info = decide(&ctx, &Tuning::default());
        assert!(info.items.iter().any(|a| a.id == "shell.last_failed"));
        // A zero exit surfaces nothing.
        ctx.app_internal.shell_exit_code = Some(0);
        assert!(decide(&ctx, &Tuning::default())
            .items
            .iter()
            .all(|a| !a.id.starts_with("shell.")));
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
            position_secs: 0,
            length_secs: 0,
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
            position_secs: 0,
            length_secs: 0,
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
            remote_url: None,
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
        // Pull is offered while coding too (dirty or not), --ff-only.
        let pull = find(&opts, "git.pull").expect("pull control while coding");
        assert!(matches!(&pull.action, AffordanceAction::Spawn { argv }
            if argv.contains(&"pull".to_string()) && argv.contains(&"--ff-only".to_string())));

        // A browser focused in the same repo dir → Browsing, NOT coding: all
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
        assert!(find(&opts, "git.pull").is_none());
    }

    #[test]
    fn git_diff_and_open_remote_and_dirty_warning() {
        let mut ctx = live_ctx();
        ctx.window.class = "code".into(); // editor → Coding
        ctx.window.pid = 1;
        ctx.git = GitContext {
            repo_root: Some("/home/max/proj".into()),
            branch: Some("main".into()),
            is_dirty: true,
            remote_url: Some("https://github.com/max/proj".into()),
        };
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        // Dirty tree is a warning; branch is ambient info.
        let dirty = find(&opts, "git.dirty").expect("dirty warning");
        assert_eq!(dirty.kind, AffordanceKind::Warning);
        assert!(find(&opts, "git.branch").is_some());
        // Diff opens a pager pipeline in a terminal, repo path shell-quoted.
        let diff = find(&opts, "git.diff").expect("diff control");
        assert!(matches!(&diff.action, AffordanceAction::Spawn { argv }
            if argv[0] == "foot" && argv.last().unwrap().contains("git -C '/home/max/proj' diff")));
        // Open-remote is the browsable URL; detail strips the scheme.
        let remote = find(&opts, "git.open_remote").expect("open-remote control");
        assert_eq!(
            remote.action,
            AffordanceAction::OpenUrl("https://github.com/max/proj".into())
        );
        assert_eq!(remote.detail, "github.com/max/proj");
        // Clean tree → no dirty warning, no diff, but pull still offered.
        ctx.git.is_dirty = false;
        let clean = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(find(&clean, "git.dirty").is_none());
        assert!(find(&clean, "git.diff").is_none());
        assert!(find(&clean, "git.pull").is_some());
    }

    #[test]
    fn git_offers_clear_outside_repo_when_detached_and_without_upstream() {
        let mut ctx = live_ctx();
        ctx.window.class = "code".into(); // Coding, so gated controls could show
        ctx.window.pid = 1;
        let roomy = Tuning {
            max_items: 12,
            ..Default::default()
        };

        // No repo at all (left the repo / not a git dir) → zero git offers.
        ctx.git = GitContext::default();
        let none = decide(&ctx, &roomy);
        assert!(!none.items.iter().any(|a| a.id.starts_with("git.")));

        // Detached HEAD: a repo_root but no branch name. The module keys off the
        // branch, so it stays silent rather than acting on an ambiguous head.
        ctx.git = GitContext {
            repo_root: Some("/home/max/proj".into()),
            branch: None,
            is_dirty: true,
            remote_url: None,
        };
        let detached = decide(&ctx, &roomy);
        assert!(!detached.items.iter().any(|a| a.id.starts_with("git.")));

        // On a branch but no remote (no upstream configured): commit/push/pull
        // still offered (push failure is a runtime concern), but there is no
        // Open-remote without a URL.
        ctx.git.branch = Some("main".into());
        let no_upstream = decide(&ctx, &roomy);
        assert!(find(&no_upstream, "git.commit").is_some());
        assert!(find(&no_upstream, "git.push").is_some());
        assert!(find(&no_upstream, "git.pull").is_some());
        assert!(find(&no_upstream, "git.open_remote").is_none());
    }

    #[test]
    fn high_focus_churn_hints_window_switching() {
        let mut ctx = live_ctx();
        // Below the threshold → quiet.
        ctx.behavior.focus_switch_velocity = 0.3;
        assert!(find(
            &decide(&ctx, &Tuning { max_items: 12, ..Default::default() }),
            "behavior.focus_churn"
        )
        .is_none());
        // Sustained rapid switching → a gentle scaffolding cue (Info).
        ctx.behavior.focus_switch_velocity = 1.4;
        let churn = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let c = find(&churn, "behavior.focus_churn").expect("focus-churn cue");
        assert_eq!(c.kind, AffordanceKind::Action);
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
        // And a call offers Do-Not-Disturb (an internal daemon action).
        let dnd = find(&opts, "audio.call_dnd").expect("dnd control in a call");
        assert_eq!(dnd.action, AffordanceAction::Daemon("toggle_dnd".into()));
        assert!(dnd.action.is_actionable());
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
            position_secs: 0,
            length_secs: 0,
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
    fn no_mpris_clears_every_media_offer() {
        let mut ctx = live_ctx();
        // A player is present → media controls exist.
        ctx.media = Some(MediaState {
            player_name: "vlc".into(),
            title: "clip".into(),
            artist: String::new(),
            is_playing: true,
            position_secs: 10,
            length_secs: 200,
        });
        let with = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(with.items.iter().any(|a| a.id.starts_with("media.")));
        // The player vanishes (no MPRIS at all) → every media.* offer clears,
        // no stale controls linger.
        ctx.media = None;
        let without = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(!without.items.iter().any(|a| a.id.starts_with("media.")));
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
    fn background_music_ranks_below_work_controls() {
        // Coding in a dirty repo WITH music playing: the git controls for what
        // you're doing lead; the background media controls are damped below them.
        let mut ctx = live_ctx();
        ctx.window.class = "foot".into();
        ctx.window.pid = 1;
        ctx.git = GitContext {
            repo_root: Some("/home/max/p".into()),
            branch: Some("main".into()),
            is_dirty: true,
            remote_url: None,
        };
        ctx.media = Some(MediaState {
            player_name: "spotify".into(),
            title: "t".into(),
            artist: "a".into(),
            is_playing: true,
            position_secs: 0,
            length_secs: 0,
        });
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let commit = find(&opts, "git.commit").unwrap().relevance;
        let playpause = find(&opts, "media.playpause").unwrap().relevance;
        assert!(commit > playpause, "work controls lead background media");

        // But when media IS the activity (watching), it keeps full weight.
        let mut watching = live_ctx();
        watching.media = ctx.media.clone();
        watching.window.class = "somerandomapp".into();
        watching.window.pid = 1; // media playing + nothing purposeful → Media
        assert_eq!(infer_activity(&watching), Activity::Media);
        let full = find(
            &decide(
                &watching,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                },
            ),
            "media.playpause",
        )
        .unwrap()
        .relevance;
        assert!(full > playpause, "foreground media keeps full weight");
    }

    #[test]
    fn editor_file_offers_open_folder() {
        let mut ctx = live_ctx();
        ctx.window.pid = 1; // a focused window (editor_file makes it Coding)
        ctx.app_internal.editor_file = Some("/home/max/proj/src/main.rs".into());
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let o = find(&opts, "editor.open_folder").expect("open-folder from editor file");
        assert_eq!(
            o.action,
            AffordanceAction::OpenUrl("/home/max/proj/src".into())
        );
        assert_eq!(o.detail, "src");
        // No editor file → nothing.
        ctx.app_internal.editor_file = None;
        assert!(find(&decide(&ctx, &Tuning::default()), "editor.open_folder").is_none());
    }

    #[test]
    fn editor_offers_run_for_scripting_languages() {
        let mut ctx = live_ctx();
        ctx.window.pid = 1;
        ctx.app_internal.editor_file = Some("/home/max/proj/app.py".into());
        ctx.app_internal.editor_language = Some("python".into());
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let r = find(&opts, "editor.run").expect("run for a python file");
        assert!(matches!(&r.action, AffordanceAction::Spawn { argv }
            if argv.iter().any(|a| a.contains("python3") && a.contains("app.py"))));
        // A compiled language has no single-file runner.
        ctx.app_internal.editor_language = Some("rust".into());
        assert!(find(
            &decide(
                &ctx,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                }
            ),
            "editor.run"
        )
        .is_none());
        assert_eq!(runner_for_language("javascript"), Some("node"));
        assert_eq!(runner_for_language("c"), None);
    }

    #[test]
    fn shell_bridge_offers_rerun_last() {
        let mut ctx = live_ctx();
        ctx.window.class = "foot".into();
        ctx.window.pid = 1;
        ctx.app_internal.shell_last_cmd = Some("cargo build".into());
        ctx.app_internal.shell_cwd = Some("/home/max/proj".into());
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let r = find(&opts, "shell.rerun").expect("rerun control in a terminal");
        assert!(matches!(&r.action, AffordanceAction::Spawn { argv }
            if argv[0] == "foot" && argv.iter().any(|a| a.contains("cargo build"))));
        // No cwd (or no cmd) → nothing.
        ctx.app_internal.shell_cwd = None;
        assert!(find(&decide(&ctx, &Tuning::default()), "shell.rerun").is_none());
    }

    #[test]
    fn shell_cwd_offers_open_files_here() {
        let mut ctx = live_ctx();
        ctx.window.class = "foot".into();
        ctx.window.pid = 1; // a terminal → Terminal activity
        ctx.app_internal.shell_cwd = Some("/home/max/photos".into());
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let f = find(&opts, "files.open_here").expect("open-files control in a terminal");
        assert_eq!(
            f.action,
            AffordanceAction::OpenUrl("/home/max/photos".into())
        );
        assert_eq!(f.detail, "photos");
        // Not while browsing.
        ctx.window.class = "firefox".into();
        assert!(find(
            &decide(
                &ctx,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                }
            ),
            "files.open_here"
        )
        .is_none());
    }

    #[test]
    fn recent_download_offers_open() {
        let mut ctx = live_ctx();
        ctx.recent_download = Some("/home/max/Downloads/report.pdf".into());
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let d = find(&opts, "downloads.open").expect("open-download control");
        assert_eq!(d.kind, AffordanceKind::Control);
        assert_eq!(d.detail, "report.pdf");
        assert_eq!(
            d.action,
            AffordanceAction::OpenUrl("/home/max/Downloads/report.pdf".into())
        );
        // A plain PDF is not an archive → no extract offer.
        assert!(find(&opts, "downloads.extract").is_none());
        // A .tar.gz download → an "Extract here" companion appears.
        ctx.recent_download = Some("/home/max/Downloads/src.tar.gz".into());
        let arc = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let x = find(&arc, "downloads.extract").expect("extract control for an archive");
        assert!(matches!(&x.action, AffordanceAction::Spawn { argv }
            if argv[0] == "file-roller" && argv.iter().any(|a| a.ends_with("src.tar.gz"))));
        // Cleared → gone, even with the source live.
        ctx.recent_download = None;
        assert!(find(
            &decide(
                &ctx,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                }
            ),
            "downloads.open"
        )
        .is_none());
    }

    #[test]
    fn coding_offers_a_terminal_here() {
        let mut ctx = live_ctx();
        ctx.window.class = "code".into(); // an editor → Coding
        ctx.window.pid = 1;
        ctx.git = GitContext {
            repo_root: Some("/home/max/proj".into()),
            branch: Some("main".into()),
            is_dirty: false,
            remote_url: None,
        };
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let t = find(&opts, "coding.terminal_here").expect("terminal-here control");
        assert!(matches!(&t.action, AffordanceAction::Spawn { argv }
            if argv[0] == "foot" && argv.contains(&"/home/max/proj".to_string())));
        // Not while merely browsing in the repo dir.
        ctx.window.class = "firefox".into();
        assert!(find(
            &decide(
                &ctx,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                }
            ),
            "coding.terminal_here"
        )
        .is_none());
    }

    #[test]
    fn copied_email_offers_compose() {
        let mut ctx = live_ctx();
        ctx.selection = TextSelection {
            highlighted_text: Some("max@golem.example".into()),
            char_count: 17,
            is_code: false,
            contains_url: false,
            is_path: false,
        };
        let opts = decide(&ctx, &Tuning::default());
        let o = find(&opts, "selection.email").expect("email control");
        assert_eq!(
            o.action,
            AffordanceAction::OpenUrl("mailto:max@golem.example".into())
        );
        // A sentence merely mentioning an address does not trigger it.
        assert!(looks_like_email("a@b.co"));
        assert!(!looks_like_email("email me at a@b.co please"));
        assert!(!looks_like_email("not-an-email"));
        assert!(!looks_like_email("a@nodot"));
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
            remote_url: None,
        };
        ctx.media = Some(MediaState {
            player_name: "vlc".into(),
            title: "t".into(),
            artist: String::new(),
            is_playing: true,
            position_secs: 0,
            length_secs: 0,
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
        // While Coding, the work controls lead and fill the small cap: git.commit
        // tops the set and background media is damped below (and here crowded
        // out of the top 4 entirely — exactly the de-clutter we want).
        assert_eq!(opts.items[0].id, "git.commit");
        assert!(opts.items.iter().any(|a| a.id.starts_with("git.")));
        // With more room, media reappears (below the git controls) — the blend
        // is preserved, just correctly ordered.
        let roomy = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(roomy.items.iter().any(|a| a.id.starts_with("media.")));
        assert!(roomy.items.iter().any(|a| a.id.starts_with("git.")));
    }

    #[test]
    fn browser_video_tab_gives_foreground_media_but_background_music_stays_damped() {
        // A video playing IN the focused browser → full-weight media controls.
        let mut ctx = live_ctx();
        ctx.window.class = "firefox".into();
        ctx.window.pid = 1;
        ctx.media = Some(MediaState {
            player_name: "Firefox".into(),
            title: "clip".into(),
            artist: String::new(),
            is_playing: true,
            position_secs: 10,
            length_secs: 100,
        });
        let vid = find(
            &decide(
                &ctx,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                },
            ),
            "media.playpause",
        )
        .unwrap()
        .relevance;

        // Same browsing context, but the player is Spotify (music in another
        // window) → damped as background.
        if let Some(m) = ctx.media.as_mut() {
            m.player_name = "Spotify".into();
        }
        let bg = find(
            &decide(
                &ctx,
                &Tuning {
                    max_items: 12,
                    ..Default::default()
                },
            ),
            "media.playpause",
        )
        .unwrap()
        .relevance;
        assert!(
            vid > bg,
            "browser video is foreground; background music is damped"
        );
    }

    #[test]
    fn fullscreen_slides_offer_slide_nav() {
        let mut ctx = live_ctx();
        ctx.window.class = "libreoffice-impress".into();
        ctx.window.pid = 1;
        ctx.window.is_fullscreen = true;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert_eq!(
            find(&opts, "slides.next").unwrap().action,
            AffordanceAction::Daemon("slide_next".into())
        );
        assert!(find(&opts, "slides.prev").is_some());
        // No "Present" while already presenting (fullscreen).
        assert!(find(&opts, "slides.present").is_none());
        // Editing the deck (windowed): no slide nav, but a one-tap "Present".
        ctx.window.is_fullscreen = false;
        let editing = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(find(&editing, "slides.next").is_none());
        let present = find(&editing, "slides.present").expect("present control while editing");
        assert_eq!(present.action, AffordanceAction::Daemon("present".into()));
        // Not for a non-presentation window.
        ctx.window.class = "firefox".into();
        assert!(find(&decide(&ctx, &Tuning::default()), "slides.present").is_none());
    }

    #[test]
    fn pdf_reader_offers_find_and_brightness() {
        let mut ctx = live_ctx();
        ctx.window.class = "org.gnome.Papers".into();
        ctx.window.pid = 1;
        ctx.metrics.has_backlight = true;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert_eq!(
            find(&opts, "reading.find").unwrap().action,
            AffordanceAction::Daemon("find_in_page".into())
        );
        assert!(find(&opts, "reading.bright_up").is_some());
        // No backlight → find only, no brightness.
        ctx.metrics.has_backlight = false;
        let no_bl = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        assert!(find(&no_bl, "reading.find").is_some());
        assert!(find(&no_bl, "reading.bright_up").is_none());
        // A non-reader window offers nothing.
        ctx.window.class = "foot".into();
        assert!(find(&decide(&ctx, &Tuning::default()), "reading.find").is_none());
    }

    #[test]
    fn browser_offers_find_in_page() {
        let mut ctx = live_ctx();
        ctx.window.class = "firefox".into();
        ctx.window.pid = 1; // a browser → Browsing
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let f = find(&opts, "browser.find").expect("find control while browsing");
        assert_eq!(f.action, AffordanceAction::Daemon("find_in_page".into()));
        let r = find(&opts, "browser.reopen_tab").expect("reopen-tab control while browsing");
        assert_eq!(r.action, AffordanceAction::Daemon("reopen_tab".into()));
        assert!(f.relevance > r.relevance, "find leads reopen");
        // Not while coding.
        ctx.window.class = "foot".into();
        let coding = decide(&ctx, &Tuning::default());
        assert!(find(&coding, "browser.find").is_none());
        assert!(find(&coding, "browser.reopen_tab").is_none());
    }

    #[test]
    fn fullscreen_offers_dnd() {
        let mut ctx = live_ctx();
        ctx.window.is_fullscreen = true;
        ctx.window.pid = 1;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let d = find(&opts, "window.fullscreen_dnd").expect("dnd while fullscreen");
        assert_eq!(d.action, AffordanceAction::Daemon("toggle_dnd".into()));
        let s = find(&opts, "window.screenshot").expect("screenshot while fullscreen");
        assert!(
            matches!(&s.action, AffordanceAction::Spawn { argv } if argv.iter().any(|a| a.contains("grim")))
        );
        ctx.window.is_fullscreen = false;
        assert!(find(&decide(&ctx, &Tuning::default()), "window.fullscreen_dnd").is_none());
        assert!(find(&decide(&ctx, &Tuning::default()), "window.screenshot").is_none());
    }

    #[test]
    fn live_camera_is_a_privacy_warning() {
        let mut ctx = live_ctx();
        ctx.metrics.is_camera_active = true;
        let opts = decide(
            &ctx,
            &Tuning {
                max_items: 12,
                ..Default::default()
            },
        );
        let cam = find(&opts, "camera.live").expect("camera warning when a video device is open");
        assert_eq!(cam.kind, AffordanceKind::Warning);
        assert!(!cam.action.is_actionable());
        // Off → nothing.
        ctx.metrics.is_camera_active = false;
        assert!(find(&decide(&ctx, &Tuning::default()), "camera.live").is_none());
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

    #[test]
    fn high_memory_offers_a_monitor_but_defers_to_cpu() {
        let mut ctx = live_ctx();
        let roomy = Tuning {
            max_items: 12,
            ..Default::default()
        };
        // High RAM, calm CPU → the memory monitor control shows.
        ctx.metrics.ram_usage_pct = 93.0;
        let mem_opts = decide(&ctx, &roomy);
        let m = find(&mem_opts, "system.high_mem").expect("high-mem control");
        assert_eq!(m.kind, AffordanceKind::Control);
        assert!(m.action.is_actionable());
        // When CPU is ALSO high, the CPU pill covers it — no duplicate monitor.
        ctx.metrics.cpu_usage_pct = 95.0;
        let opts = decide(&ctx, &roomy);
        assert!(find(&opts, "system.high_cpu").is_some());
        assert!(find(&opts, "system.high_mem").is_none());
        // Calm memory → nothing.
        ctx.metrics.cpu_usage_pct = 10.0;
        ctx.metrics.ram_usage_pct = 55.0;
        assert!(find(&decide(&ctx, &roomy), "system.high_mem").is_none());
    }
}

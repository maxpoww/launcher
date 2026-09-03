//! Activity awareness: the mind's read of *what the user is doing*.
//!
//! Raw context (a window class, a git branch, a live mic) is not yet
//! understanding. [`infer_activity`] folds the whole [`ContextState`] into a
//! single high-level [`Activity`] — the situation the mind reasons about. It is
//! deliberately a pure, ordered classifier: higher-priority situations (you're
//! in a call; you're reading docs) win over ambient ones (something's playing).
//!
//! This is the seat of "context/activity awareness": every decision the mind
//! makes can be conditioned on the activity, so options fit the situation and
//! the wrong ones are cleared away (pillars 2 & 3).

use serde::Serialize;

use crate::state::ContextState;

/// What the user is doing right now, at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum Activity {
    /// Nothing focused — an empty workspace.
    Idle,
    /// A live microphone: a call or meeting is the dominant situation.
    Communication,
    /// Reading documentation in the browser (MDN / StackOverflow / GitHub …).
    Reading,
    /// A browser is the focus (general web).
    Browsing,
    /// Writing code — an editor is focused, or a terminal where development
    /// work is actually happening (not merely one sitting in a repo).
    Coding,
    /// A terminal is focused, not clearly a coding session.
    Terminal,
    /// Media is playing and nothing more specific is going on.
    Media,
    /// Focused on an app we don't classify.
    #[default]
    Unknown,
}

/// Window classes we treat as code editors (substring match, lower-cased).
const EDITORS: &[&str] = &[
    "code",
    "vscodium",
    "nvim",
    "vim",
    "neovide",
    "emacs",
    "zed",
    "sublime_text",
    "helix",
    "kate",
    "jetbrains",
    "idea",
    "pycharm",
    "goland",
    "clion",
    "webstorm",
    "rustrover",
];
/// Terminal emulators.
const TERMINALS: &[&str] = &[
    "foot",
    "alacritty",
    "kitty",
    "wezterm",
    "gnome-terminal",
    "konsole",
    "xterm",
    "urxvt",
    "terminator",
    "st",
    "tilix",
    "ghostty",
];
/// Web browsers.
const BROWSERS: &[&str] = &[
    "firefox",
    "chromium",
    "chrome",
    "google-chrome",
    "brave",
    "librewolf",
    "vivaldi",
    "edge",
    "zen",
];

fn matches_any(class_lower: &str, set: &[&str]) -> bool {
    set.iter().any(|k| class_lower.contains(k))
}

/// Whether a browser window title reads like a documentation / reference page —
/// the reading-mode heuristic used in place of a browser bridge. Kept to strong
/// markers so a random page rarely trips it (a false positive only adds the mild
/// reading offers — find + brightness — which are harmless on any page).
fn title_looks_like_docs(title: &str) -> bool {
    let t = title.to_lowercase();
    const MARKERS: &[&str] = &[
        "documentation",
        "readthedocs",
        "read the docs",
        "mdn",
        "stack overflow",
        "wikipedia",
        "reference manual",
        "man page",
        "api reference",
        "developer guide",
        "user guide",
        " docs",
        " — docs",
    ];
    MARKERS.iter().any(|m| t.contains(m))
}

/// Whether the last command run in the focused shell is development work — the
/// evidence that turns a terminal from [`Activity::Terminal`] into
/// [`Activity::Coding`]. Deliberately a whitelist of tools you *build* with:
/// anything unrecognised (a monitor, a pager, ssh, cd, ls) leaves the terminal
/// alone, because the cost of a false Coding is a bar full of git controls the
/// user did not ask for, while the cost of a false Terminal is one extra
/// keystroke to run git yourself.
fn looks_like_dev_work(last_cmd: &str) -> bool {
    // The command word, after any leading env assignments (FOO=1 cargo …) and
    // a `sudo`/`doas` prefix; a path (…/bin/cargo) reduces to its file name.
    let word = last_cmd
        .split_whitespace()
        .find(|w| !w.contains('=') && !matches!(*w, "sudo" | "doas" | "env" | "nice" | "time"))
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    const DEV_TOOLS: &[&str] = &[
        "git",
        "cargo",
        "rustc",
        "make",
        "cmake",
        "meson",
        "ninja",
        "gcc",
        "g++",
        "clang",
        "go",
        "zig",
        "npm",
        "pnpm",
        "yarn",
        "node",
        "deno",
        "python",
        "python3",
        "pip",
        "ruby",
        "gem",
        "mvn",
        "gradle",
        "dotnet",
        "nix",
        "nix-build",
        "nix-shell",
        "nixos-rebuild",
        "docker",
        "podman",
        "pytest",
        "tox",
        "rustfmt",
        "clippy-driver",
        "vim",
        "nvim",
        "emacs",
        "hx",
        "kak",
        "helix",
    ];
    DEV_TOOLS.contains(&word)
}

/// Classify the current activity from a context snapshot. Ordered by priority:
/// the first situation that fits wins.
pub fn infer_activity(ctx: &ContextState) -> Activity {
    // Nothing focused.
    if ctx.window.pid == 0 && ctx.window.class.is_empty() {
        return Activity::Idle;
    }
    // A live mic dominates everything: you're in a call.
    if ctx.audio.is_mic_active {
        return Activity::Communication;
    }
    let class = ctx.window.class.to_lowercase();
    // Reading docs is a more specific browser state than plain browsing.
    if ctx.app_internal.is_reading_docs {
        return Activity::Reading;
    }
    if matches_any(&class, BROWSERS) || ctx.app_internal.browser_url.is_some() {
        // Reading docs is a more specific browser state. Absent a browser bridge
        // (blocked: an extension needs a store upload, and enabling the main
        // browser's remote-debugging port is a security exposure), infer it from
        // the window title the compositor already gives us — documentation sites
        // put recognisable markers there.
        if title_looks_like_docs(&ctx.window.title) {
            return Activity::Reading;
        }
        return Activity::Browsing;
    }
    // An editor is coding outright; a terminal has to earn it (just below).
    if matches_any(&class, EDITORS) || ctx.app_internal.editor_file.is_some() {
        return Activity::Coding;
    }
    if matches_any(&class, TERMINALS) {
        // A terminal is Coding only when the user is DOING development in it —
        // not merely because its working directory happens to be a repo. That
        // rule filled the bar with git controls while Max sat watching btop
        // inside ~/Golem (2026-09-02: "the options engine is shit, im on btop
        // it show me 5 options for git hub"). A directory is not an intention.
        //
        // The evidence we accept is the last command the shell bridge saw: a
        // dev tool means dev work. A monitor (btop/top/htop), a pager, ssh, or
        // no command at all leaves it a plain Terminal, where the git module
        // stays out of the way.
        return if ctx.app_internal.editor_file.is_some()
            || ctx
                .app_internal
                .shell_last_cmd
                .as_deref()
                .is_some_and(looks_like_dev_work)
        {
            Activity::Coding
        } else {
            Activity::Terminal
        };
    }
    // Media is only the activity if nothing more purposeful is happening.
    if ctx.media.as_ref().is_some_and(|m| m.is_playing) {
        return Activity::Media;
    }
    Activity::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActiveWindow, AppInternalContext, AudioState, GitContext, MediaState};

    fn focused(class: &str) -> ContextState {
        ContextState {
            window: ActiveWindow {
                class: class.into(),
                pid: 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn empty_is_idle() {
        assert_eq!(infer_activity(&ContextState::default()), Activity::Idle);
    }

    #[test]
    fn live_mic_beats_everything() {
        let mut ctx = focused("foot");
        ctx.audio = AudioState {
            is_mic_active: true,
            ..Default::default()
        };
        ctx.media = Some(MediaState {
            is_playing: true,
            ..Default::default()
        });
        assert_eq!(infer_activity(&ctx), Activity::Communication);
    }

    #[test]
    fn editor_is_coding() {
        assert_eq!(infer_activity(&focused("Code")), Activity::Coding);
        assert_eq!(infer_activity(&focused("neovide")), Activity::Coding);
    }

    /// A repo working directory is NOT an intention: watching btop in a
    /// terminal that happens to sit in a repo must stay Terminal, or the bar
    /// fills with git controls nobody asked for (Max, 2026-09-02). It takes an
    /// actual dev command — or an open editor file — to make it Coding.
    #[test]
    fn terminal_needs_dev_work_not_just_a_repo_to_be_coding() {
        let mut ctx = focused("foot");
        assert_eq!(infer_activity(&ctx), Activity::Terminal);
        ctx.git = GitContext {
            branch: Some("main".into()),
            ..Default::default()
        };
        assert_eq!(
            infer_activity(&ctx),
            Activity::Terminal,
            "cwd in a repo alone is not coding"
        );
        for monitoring in [
            "btop",
            "htop",
            "top -d 1",
            "less NOTES.md",
            "ssh box",
            "ls -la",
        ] {
            ctx.app_internal.shell_last_cmd = Some(monitoring.into());
            assert_eq!(
                infer_activity(&ctx),
                Activity::Terminal,
                "'{monitoring}' is not development"
            );
        }
        for dev in [
            "git status",
            "cargo test --workspace",
            "sudo nixos-rebuild build-vm",
            "RUST_LOG=debug cargo run",
            "/run/current-system/sw/bin/nix build",
            "nvim src/main.rs",
        ] {
            ctx.app_internal.shell_last_cmd = Some(dev.into());
            assert_eq!(
                infer_activity(&ctx),
                Activity::Coding,
                "'{dev}' is dev work"
            );
        }
        // An open editor file counts even with no shell command seen.
        ctx.app_internal.shell_last_cmd = None;
        ctx.app_internal.editor_file = Some("/home/max/x.rs".into());
        assert_eq!(infer_activity(&ctx), Activity::Coding);
    }

    #[test]
    fn browser_is_browsing_unless_reading_docs() {
        assert_eq!(infer_activity(&focused("firefox")), Activity::Browsing);
        let mut ctx = focused("firefox");
        ctx.app_internal = AppInternalContext {
            is_reading_docs: true,
            ..Default::default()
        };
        assert_eq!(infer_activity(&ctx), Activity::Reading);
    }

    #[test]
    fn browser_doc_title_infers_reading_without_a_bridge() {
        // A documentation title flips Browsing → Reading purely from the
        // window title (no browser bridge), so a docs tab gets the reading
        // offers (find, brightness).
        let mut ctx = focused("firefox");
        for docish in [
            "array — Python documentation",
            "Array - JavaScript | MDN",
            "regex - How to ... - Stack Overflow",
            "requests: HTTP for Humans — Docs",
            "Rust By Example - User Guide",
        ] {
            ctx.window.title = docish.into();
            assert_eq!(infer_activity(&ctx), Activity::Reading, "{docish:?}");
        }
        // An ordinary page stays Browsing.
        ctx.window.title = "Cat videos - YouTube".into();
        assert_eq!(infer_activity(&ctx), Activity::Browsing);
        // The title heuristic only applies to browsers, not other apps.
        let mut term = focused("foot");
        term.window.title = "man page".into();
        assert_eq!(infer_activity(&term), Activity::Terminal);
    }

    #[test]
    fn doc_title_marker_matching() {
        assert!(title_looks_like_docs("NumPy Documentation"));
        assert!(title_looks_like_docs("something - MDN"));
        assert!(!title_looks_like_docs("My Cool Blog Post"));
        assert!(!title_looks_like_docs(""));
    }

    #[test]
    fn background_media_is_media_only_when_nothing_else() {
        let mut ctx = focused("somerandomapp");
        ctx.media = Some(MediaState {
            is_playing: true,
            ..Default::default()
        });
        assert_eq!(infer_activity(&ctx), Activity::Media);
        // But a code editor with music playing is still Coding.
        let mut coding = focused("Code");
        coding.media = Some(MediaState {
            is_playing: true,
            ..Default::default()
        });
        assert_eq!(infer_activity(&coding), Activity::Coding);
    }
}

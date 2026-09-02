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
    /// Writing code — an editor is focused, or a terminal inside a repository.
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
        return Activity::Browsing;
    }
    // An editor is coding; a terminal is coding only if it's in a repo.
    if matches_any(&class, EDITORS) || ctx.app_internal.editor_file.is_some() {
        return Activity::Coding;
    }
    if matches_any(&class, TERMINALS) {
        return if ctx.git.branch.is_some() {
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

    #[test]
    fn terminal_in_repo_is_coding_else_terminal() {
        let mut ctx = focused("foot");
        assert_eq!(infer_activity(&ctx), Activity::Terminal);
        ctx.git = GitContext {
            branch: Some("main".into()),
            ..Default::default()
        };
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

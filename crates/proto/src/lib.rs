//! Wire types shared by the waverunner daemon and the `waverunner-ctl`
//! control client.
//!
//! The IPC protocol is deliberately trivial: a line-oriented plain-text
//! protocol over a Unix domain socket. The client connects, writes exactly
//! one command line, the daemon answers with exactly one response line and
//! closes the connection. No framing, no async, no serialization crate.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// Errors produced when parsing protocol messages received over the socket.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The input line did not match any known command.
    #[error("unknown command: {0:?}")]
    UnknownCommand(String),
    /// The input line did not match any known response.
    #[error("unknown response: {0:?}")]
    UnknownResponse(String),
}

/// A command sent from the client to the daemon.
///
/// The launcher has three rest states: hidden, dock (a slim bar at the
/// bottom edge), and open (the full popup). `toggle`/`show`/`hide` move
/// between hidden and dock; `expand`/`collapse` move between dock and
/// open (normally driven by scrolling on the dock, exposed here for
/// scripting and testing).
/// Note: not `Copy` — the overview verbs carry a payload (a window title,
/// a size); clone at the few call sites that need the value twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the dock if hidden, hide the launcher entirely otherwise.
    Toggle,
    /// Show the dock (no-op unless hidden).
    Show,
    /// Hide the launcher entirely (no-op if already hidden).
    Hide,
    /// Slide from dock to the fully open popup (no-op unless docked).
    Expand,
    /// Slide from the open popup back down to the dock (no-op unless open).
    Collapse,
    /// Debug/verification: force-open the clipboard history box, so the OPTIONS
    /// surfaces (normally pointer-only) can be screenshotted deterministically.
    DebugClip,
    /// Debug/verification: force-open the clipboard box on the newest row's
    /// metadata detail view.
    DebugClipDetail,
    /// Debug/verification: force-open the clipboard box with the type-to-search
    /// field out and this query typed into it (empty = the bare field), so the
    /// filtered list can be screenshotted without a keyboard.
    DebugClipSearch(String),
    /// Debug/verification: force-open the clipboard box on the emoji picker,
    /// optionally with this search query typed into the box's field.
    DebugEmoji(String),
    /// Debug/verification: force-open the notification history box.
    DebugNotif,
    /// Debug/verification: stand a sticky OPTION on a concealed bar (the
    /// fullscreen control plus its doorway) so §4 can be screenshotted without
    /// a pointer and a real fullscreen window.
    DebugSticky,
    /// Debug/verification: force-open the clipboard box on the dictionary
    /// "define a word" panel, pre-filled with a sample query.
    DebugDict,
    /// Debug/verification: log the Mind's current OptionSet (activity + the
    /// ranked offers with their ids/actions) to the journal, so the OPTIONS
    /// engine's live decisions can be inspected without a GUI click.
    DebugOptions,
    /// Debug/verification: force the hover state onto the first dynamic OPTION
    /// pill and redraw, so its discoverability tooltip can be screenshotted
    /// without a pointer (there's no headless cursor warp on this compositor).
    DebugHoverOption,
    /// Debug/verification: toggle the sunset eye-protection prompt (the
    /// current-task pill's expansion into "the sun is set…" + [turn on]) as if
    /// the Mind had offered it, so the UX can be demoed before real sunset.
    DebugSunset,
    /// Debug/verification: toggle the settings BOX of whichever module is on the
    /// current-task pill (the gear's own act). The topbar is pointer-only on this
    /// compositor, so this is how an open panel gets screenshotted at all.
    DebugModuleBox,
    /// Debug/verification: raise the gear's readout, then its panel, then put both away — or, with
    /// a page number, press that button directly (1 = the gear, 2.. = each
    /// reading in the order they are drawn). The pointer-free way to exercise
    /// the pages.
    DebugStats(Option<usize>),
    /// Golem as a floating window manager instead of a tiling one: `on`, `off`
    /// or `toggle` (empty = toggle). The gear's page-1 switch sends the same
    /// thing; a verb so it can be bound to a key and driven without a pointer.
    FloatMode(String),
    /// Trigger the dynamic OPTION offer with this affordance id (e.g.
    /// `media.playpause`, `git.commit`) — the same action a click on its pill
    /// runs. Exposed for scripting and for verifying the action end to end.
    OptionsTrigger(String),
    /// The compositor overview opened: conceal every waverunner surface
    /// (topbar + dock) and ignore reveals until `OverviewOff`.
    OverviewOn,
    /// The compositor overview closed: surfaces may return.
    OverviewOff,
    /// A window resize drag began (waveview watches the compositor's drag
    /// state): the topbar shows the live size readout until the drop.
    ResizeDragOn,
    /// The resize drag ended.
    ResizeDragOff,
    /// The user interacted with the focused window — a key, click, or
    /// scroll aimed at it (waveview watches compositor input and sends one
    /// per window visit). Commits an in-flight focus walk and earns the
    /// window its usage point: only windows actually worked in rank.
    Interacted,
    /// Cycle focus through the current workspace's windows, most-used
    /// first (the current-task pill's left click; bindable as Super+Tab).
    FocusNext,
    /// Cycle focus into the other workspaces' windows, most-used first
    /// (the pill's right click).
    FocusOther,
    /// Toggle Golem pseudo (tag + proportional size + framed) on the
    /// focused window — the topbar's square pill, bindable as Super+P.
    PseudoToggle,
    /// Overview: the window under the pointer changed — the topbar's
    /// current-task pill shows this title while the overview owns the
    /// screen. Empty payload = nothing hovered (back to the focused
    /// window's title).
    OverviewHover(String),
    /// Overview: a thumbnail is being resized at this size (`"1240x1000"`),
    /// shown as the pill's live readout. Empty payload = resize ended.
    OverviewResize(String),
    /// STAGE mode on/off (bindable as Super+Enter): one task alone on screen at
    /// the stage rect, the deck of every other task below it. Toggling off puts
    /// the desktop back exactly as it was, focus included.
    StageToggle,
    /// While staged: put this window on the stage, by address (`0x…`). Empty
    /// payload is a no-op. The deck's click will call this; it is a verb so the
    /// mode can be driven and verified before the deck exists.
    StageShow(String),
    /// Switch the stage between showing one task and showing one whole desk
    /// (per-window / per-workspace). The bar's stage-mode pill does the same;
    /// this is the way to drive it without a pointer, and it works off the stage
    /// too, where it chooses the mode the stage will next open in.
    StageMode,
    /// While staged: a 3/4-finger swipe is walking the deck's border. The
    /// payload is the gesture's **total** travel in logical px so far, positive
    /// rightward (`"-243.5"`). Sent by the waveview plugin, which owns the
    /// trackpad while the stage is up; nothing is staged until the end below, so
    /// scrubbing across the deck costs no window switches.
    ///
    /// Absolute rather than incremental so a message that arrives late or not at
    /// all cannot leave the border a tile out of step.
    StageSwipe(String),
    /// The swipe's fingers left the pad: stage whatever the border landed on.
    /// The payload is the final total travel; an empty or unreadable one commits
    /// the border where it stands.
    StageSwipeEnd(String),
    /// While staged: put a numbered tile on the stage (`Super+N`, bound inside
    /// the stage submap). The payload is the number as typed — the n-th tile in
    /// task mode, workspace n in desk mode, whichever the deck is showing. A
    /// number with no tile does nothing.
    StagePick(String),
    /// Put the focused window into a window mode: `tiled`, `float`, `pseudo` or
    /// `fullscreen` — the same four the bar's pills offer, and the same toggle
    /// rule (asking for the mode it is already in returns it to the layout).
    ///
    /// Exists so things outside the daemon can use Golem's mode machinery
    /// rather than the compositor's: the titlebar's "back to the layout" button
    /// goes through here, which is what makes the solitary-pseudo rule notice a
    /// window rejoining the layout.
    WindowMode(String),
    /// The waveview plugin minimized a window into the dock (macOS style).
    /// Payload: `<addr> <ws> <aspect> <class> <path> <title…>` — window
    /// address (`0x…`), workspace id (informational; the plugin owns
    /// restore), the window's width/height ratio (its tile shape), its
    /// Hyprland class (one token, `?` if unknown — resolves the corner app
    /// badge), the raw-RGBA thumbnail file it rendered, then the window
    /// title, which MAY CONTAIN SPACES (everything after the path). Stays one
    /// opaque String here; `minimized.rs` on the daemon splits the fields.
    MinAdd(String),
    /// The minimized window came back or died: drop its dock tile. Payload
    /// is the window address; an unknown one is a no-op — the daemon's own
    /// `closewindow` eviction may have beaten this message to it.
    MinDel(String),
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Toggle => f.write_str("toggle"),
            Command::Show => f.write_str("show"),
            Command::Hide => f.write_str("hide"),
            Command::Expand => f.write_str("expand"),
            Command::Collapse => f.write_str("collapse"),
            Command::DebugClip => f.write_str("debug-clip"),
            Command::DebugClipDetail => f.write_str("debug-clip-detail"),
            Command::DebugClipSearch(q) => write!(f, "debug-clip-search {q}"),
            Command::DebugEmoji(q) => write!(f, "debug-emoji {q}"),
            Command::DebugNotif => f.write_str("debug-notif"),
            Command::DebugSticky => f.write_str("debug-sticky"),
            Command::DebugDict => f.write_str("debug-dict"),
            Command::DebugOptions => f.write_str("debug-options"),
            Command::DebugHoverOption => f.write_str("debug-hover-option"),
            Command::DebugSunset => f.write_str("debug-sunset"),
            Command::DebugModuleBox => f.write_str("debug-module-box"),
            Command::DebugStats(None) => f.write_str("debug-stats"),
            Command::DebugStats(Some(p)) => write!(f, "debug-stats {p}"),
            Command::FloatMode(m) => write!(f, "float-mode {m}"),
            Command::OptionsTrigger(id) => write!(f, "options-trigger {id}"),
            Command::OverviewOn => f.write_str("overview-on"),
            Command::OverviewOff => f.write_str("overview-off"),
            Command::ResizeDragOn => f.write_str("resize-drag-on"),
            Command::ResizeDragOff => f.write_str("resize-drag-off"),
            Command::Interacted => f.write_str("interacted"),
            Command::FocusNext => f.write_str("focus-next"),
            Command::FocusOther => f.write_str("focus-other"),
            Command::PseudoToggle => f.write_str("pseudo-toggle"),
            Command::OverviewHover(t) => write!(f, "overview-hover {t}"),
            Command::OverviewResize(s) => write!(f, "overview-resize {s}"),
            Command::StageToggle => f.write_str("stage-toggle"),
            Command::StageShow(a) => write!(f, "stage-show {a}"),
            Command::StageMode => f.write_str("stage-mode"),
            Command::StageSwipe(dx) => write!(f, "stage-swipe {dx}"),
            Command::StageSwipeEnd(dx) => write!(f, "stage-swipe-end {dx}"),
            Command::StagePick(n) => write!(f, "stage-pick {n}"),
            Command::WindowMode(m) => write!(f, "window-mode {m}"),
            Command::MinAdd(p) => write!(f, "min-add {p}"),
            Command::MinDel(a) => write!(f, "min-del {a}"),
        }
    }
}

impl FromStr for Command {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Payload verbs first: the rest of the line is the value, so only
        // the trailing newline is stripped (a window title keeps its own
        // spacing).
        let line = s.trim_end_matches(['\n', '\r']);
        for (verb, wrap) in [
            (
                "overview-hover",
                Command::OverviewHover as fn(String) -> Command,
            ),
            (
                "overview-resize",
                Command::OverviewResize as fn(String) -> Command,
            ),
            (
                "options-trigger",
                Command::OptionsTrigger as fn(String) -> Command,
            ),
            (
                "debug-clip-search",
                Command::DebugClipSearch as fn(String) -> Command,
            ),
            ("debug-emoji", Command::DebugEmoji as fn(String) -> Command),
            ("stage-show", Command::StageShow as fn(String) -> Command),
            // Before its own prefix: `stage-swipe` would otherwise be tried
            // against `stage-swipe-end …` first. (It declines — the remainder
            // starts with `-`, not a space — but the order says the intent.)
            (
                "stage-swipe-end",
                Command::StageSwipeEnd as fn(String) -> Command,
            ),
            ("stage-swipe", Command::StageSwipe as fn(String) -> Command),
            ("stage-pick", Command::StagePick as fn(String) -> Command),
            ("window-mode", Command::WindowMode as fn(String) -> Command),
            ("float-mode", Command::FloatMode as fn(String) -> Command),
            ("min-add", Command::MinAdd as fn(String) -> Command),
            ("min-del", Command::MinDel as fn(String) -> Command),
        ] {
            if let Some(rest) = line.strip_prefix(verb) {
                // `verb` alone (or `verb ` + text) — anything else is a
                // different command that merely shares the prefix.
                if rest.is_empty() {
                    return Ok(wrap(String::new()));
                }
                if let Some(payload) = rest.strip_prefix(' ') {
                    return Ok(wrap(payload.to_owned()));
                }
            }
        }
        match s.trim() {
            "toggle" => Ok(Command::Toggle),
            "show" => Ok(Command::Show),
            "hide" => Ok(Command::Hide),
            "expand" => Ok(Command::Expand),
            "collapse" => Ok(Command::Collapse),
            "debug-clip" => Ok(Command::DebugClip),
            "debug-clip-detail" => Ok(Command::DebugClipDetail),
            "debug-notif" => Ok(Command::DebugNotif),
            "debug-sticky" => Ok(Command::DebugSticky),
            "debug-dict" => Ok(Command::DebugDict),
            "debug-options" => Ok(Command::DebugOptions),
            "debug-hover-option" => Ok(Command::DebugHoverOption),
            "debug-sunset" => Ok(Command::DebugSunset),
            "debug-module-box" => Ok(Command::DebugModuleBox),
            // `debug-stats` alone cycles; `debug-stats N` presses button N. Its
            // payload is a NUMBER, not a string, so it cannot ride the
            // `fn(String) -> Command` table above and is matched here instead.
            "debug-stats" => Ok(Command::DebugStats(None)),
            s if s.starts_with("debug-stats ") => Ok(Command::DebugStats(
                s.trim_start_matches("debug-stats ").trim().parse().ok(),
            )),
            "overview-on" => Ok(Command::OverviewOn),
            "overview-off" => Ok(Command::OverviewOff),
            "resize-drag-on" => Ok(Command::ResizeDragOn),
            "resize-drag-off" => Ok(Command::ResizeDragOff),
            "interacted" => Ok(Command::Interacted),
            "pseudo-toggle" => Ok(Command::PseudoToggle),
            "focus-next" => Ok(Command::FocusNext),
            "focus-other" => Ok(Command::FocusOther),
            "stage-toggle" => Ok(Command::StageToggle),
            "stage-mode" => Ok(Command::StageMode),
            other => Err(ParseError::UnknownCommand(other.to_owned())),
        }
    }
}

/// Every verb, for the CLI's usage text — one entry per [`Command`] variant,
/// payload verbs shown with their argument. The client renders its usage from
/// this table, so it can never again fall out of date the way the hand-written
/// string did (found live 2026-09-02: it stopped at `debug-dict`, hiding
/// `debug-options`/`options-trigger`/… from anyone at the prompt). The proto
/// tests walk this table against the parser AND walk every variant against
/// this table, so a new `Command` that misses it fails the build's tests.
pub const USAGE_VERBS: &[&str] = &[
    "toggle",
    "show",
    "hide",
    "expand",
    "collapse",
    "debug-clip",
    "debug-clip-detail",
    "debug-clip-search [query]",
    "debug-emoji [query]",
    "debug-notif",
    "debug-sticky",
    "debug-dict",
    "debug-options",
    "debug-hover-option",
    "debug-sunset",
    "debug-module-box",
    "debug-stats",
    "options-trigger <id>",
    "overview-on",
    "overview-off",
    "resize-drag-on",
    "resize-drag-off",
    "interacted",
    "focus-next",
    "focus-other",
    "pseudo-toggle",
    "overview-hover [title]",
    "overview-resize [WxH]",
    "stage-toggle",
    "stage-show <address>",
    "stage-mode",
    "stage-swipe <dx>",
    "stage-swipe-end <dx>",
    "stage-pick <n>",
    "window-mode <tiled|float|pseudo|fullscreen>",
    "float-mode <on|off|toggle>",
    "min-add <addr> <ws> <aspect> <class> <path> <title…>",
    "min-del <addr>",
];

/// A response sent from the daemon back to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// The command was accepted.
    Ok,
    /// The command failed; the payload is a human-readable reason.
    Err(String),
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Response::Ok => f.write_str("ok"),
            Response::Err(reason) => write!(f, "err {reason}"),
        }
    }
}

impl FromStr for Response {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let s = s.trim();
        if s == "ok" {
            return Ok(Response::Ok);
        }
        if let Some(reason) = s.strip_prefix("err ") {
            return Ok(Response::Err(reason.to_owned()));
        }
        Err(ParseError::UnknownResponse(s.to_owned()))
    }
}

/// Path of the daemon's control socket.
///
/// `$XDG_RUNTIME_DIR/waverunner.sock`, falling back to
/// `/tmp/waverunner-$UID.sock` when `XDG_RUNTIME_DIR` is unset.
pub fn socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("waverunner.sock");
    }
    PathBuf::from(format!("/tmp/waverunner-{}.sock", uid_fallback()))
}

/// Minimal `getuid` without a libc dependency: read it from /proc.
///
/// Only used on the `XDG_RUNTIME_DIR`-less fallback path, which should not
/// happen in a real Wayland session.
fn uid_fallback() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One instance of EVERY command variant. The inner match is the
    /// exhaustiveness tripwire: adding a `Command` variant breaks this
    /// function's compile until it is added here (and thus to the round-trip
    /// and usage-coverage tests below).
    fn all_commands() -> Vec<Command> {
        fn _exhaustive(c: &Command) {
            match c {
                Command::Toggle
                | Command::Show
                | Command::Hide
                | Command::Expand
                | Command::Collapse
                | Command::DebugClip
                | Command::DebugClipDetail
                | Command::DebugClipSearch(_)
                | Command::DebugEmoji(_)
                | Command::DebugNotif
                | Command::DebugSticky
                | Command::DebugDict
                | Command::DebugOptions
                | Command::DebugHoverOption
                | Command::DebugSunset
                | Command::DebugModuleBox
                | Command::DebugStats(_)
                | Command::FloatMode(_)
                | Command::OptionsTrigger(_)
                | Command::OverviewOn
                | Command::OverviewOff
                | Command::ResizeDragOn
                | Command::ResizeDragOff
                | Command::Interacted
                | Command::FocusNext
                | Command::FocusOther
                | Command::PseudoToggle
                | Command::OverviewHover(_)
                | Command::OverviewResize(_)
                | Command::StageToggle
                | Command::StageShow(_)
                | Command::StageMode
                | Command::StageSwipe(_)
                | Command::StageSwipeEnd(_)
                | Command::StagePick(_)
                | Command::WindowMode(_)
                | Command::MinAdd(_)
                | Command::MinDel(_) => (),
            }
        }
        vec![
            Command::Toggle,
            Command::Show,
            Command::Hide,
            Command::Expand,
            Command::Collapse,
            Command::DebugClip,
            Command::DebugClipDetail,
            Command::DebugClipSearch("shot".into()),
            Command::DebugEmoji("happy".into()),
            Command::DebugNotif,
            Command::DebugSticky,
            Command::DebugDict,
            Command::DebugOptions,
            Command::DebugHoverOption,
            Command::DebugSunset,
        Command::DebugModuleBox,
        Command::DebugStats(None),
        Command::FloatMode(String::new()),
            Command::OptionsTrigger("media.playpause".into()),
            Command::OverviewOn,
            Command::OverviewOff,
            Command::ResizeDragOn,
            Command::ResizeDragOff,
            Command::Interacted,
            Command::FocusNext,
            Command::FocusOther,
            Command::PseudoToggle,
            Command::OverviewHover("A Window Title".into()),
            Command::OverviewResize("1240x1000".into()),
            Command::StageToggle,
            Command::StageShow("0x5c351e2e7660".into()),
            Command::StageMode,
            Command::StageSwipe("-243.5".into()),
            Command::StageSwipeEnd("-243.5".into()),
            Command::StagePick("3".into()),
            Command::WindowMode("pseudo".into()),
            // The title tail may contain spaces — the round-trip must keep it.
            Command::MinAdd(
                "0x5c351e2e7660 3 1.6296 firefox /run/user/1000/min.rgba A Window Title".into(),
            ),
            Command::MinDel("0x5c351e2e7660".into()),
        ]
    }

    /// Display ↔ FromStr round-trips over every variant, payloads included.
    #[test]
    fn command_roundtrip() {
        for cmd in all_commands() {
            assert_eq!(cmd.to_string().parse::<Command>().unwrap(), cmd, "{cmd}");
        }
    }

    /// The usage table and the parser cover each other exactly: every table
    /// entry parses (payload verbs with a dummy payload), and every variant's
    /// verb appears in the table — so the CLI's help can never go stale again.
    #[test]
    fn usage_table_and_parser_cover_each_other() {
        for entry in USAGE_VERBS {
            let verb = entry.split_whitespace().next().unwrap();
            let line = if entry.contains(' ') {
                format!("{verb} x")
            } else {
                verb.to_owned()
            };
            assert!(
                line.parse::<Command>().is_ok(),
                "usage entry {entry:?} does not parse"
            );
        }
        for cmd in all_commands() {
            let s = cmd.to_string();
            let verb = s.split_whitespace().next().unwrap();
            assert!(
                USAGE_VERBS
                    .iter()
                    .any(|e| e.split_whitespace().next() == Some(verb)),
                "verb {verb:?} missing from USAGE_VERBS"
            );
        }
    }

    #[test]
    fn options_trigger_roundtrips_with_payload() {
        let cmd = Command::OptionsTrigger("media.playpause".into());
        assert_eq!(cmd.to_string(), "options-trigger media.playpause");
        assert_eq!(cmd.to_string().parse::<Command>().unwrap(), cmd);
        // Bare verb → empty id.
        assert_eq!(
            "options-trigger".parse::<Command>().unwrap(),
            Command::OptionsTrigger(String::new())
        );
    }

    #[test]
    fn command_tolerates_whitespace() {
        assert_eq!(" toggle\n".parse::<Command>().unwrap(), Command::Toggle);
    }

    #[test]
    fn unknown_command_is_error() {
        assert!("frobnicate".parse::<Command>().is_err());
    }

    #[test]
    fn response_roundtrip() {
        for resp in [Response::Ok, Response::Err("boom".into())] {
            assert_eq!(resp.to_string().parse::<Response>().unwrap(), resp);
        }
    }
}

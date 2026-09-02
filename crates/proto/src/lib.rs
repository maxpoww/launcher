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
    /// Debug/verification: force-open the notification history box.
    DebugNotif,
    /// Debug/verification: force-open the clipboard box on the dictionary
    /// "define a word" panel, pre-filled with a sample query.
    DebugDict,
    /// Debug/verification: log the Mind's current OptionSet (activity + the
    /// ranked offers with their ids/actions) to the journal, so the OPTIONS
    /// engine's live decisions can be inspected without a GUI click.
    DebugOptions,
    /// Debug/verification: force-open the media transport box (when a player is
    /// active), so it can be screenshotted without a pointer click.
    DebugMediaBox,
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
            Command::DebugNotif => f.write_str("debug-notif"),
            Command::DebugDict => f.write_str("debug-dict"),
            Command::DebugOptions => f.write_str("debug-options"),
            Command::DebugMediaBox => f.write_str("debug-media-box"),
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
            "debug-dict" => Ok(Command::DebugDict),
            "debug-options" => Ok(Command::DebugOptions),
            "debug-media-box" => Ok(Command::DebugMediaBox),
            "overview-on" => Ok(Command::OverviewOn),
            "overview-off" => Ok(Command::OverviewOff),
            "resize-drag-on" => Ok(Command::ResizeDragOn),
            "resize-drag-off" => Ok(Command::ResizeDragOff),
            "interacted" => Ok(Command::Interacted),
            "pseudo-toggle" => Ok(Command::PseudoToggle),
            "focus-next" => Ok(Command::FocusNext),
            "focus-other" => Ok(Command::FocusOther),
            other => Err(ParseError::UnknownCommand(other.to_owned())),
        }
    }
}

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

    #[test]
    fn command_roundtrip() {
        for cmd in [
            Command::Toggle,
            Command::Show,
            Command::Hide,
            Command::Expand,
            Command::Collapse,
            Command::DebugClip,
            Command::DebugClipDetail,
            Command::DebugNotif,
            Command::DebugDict,
            Command::DebugOptions,
            Command::OverviewOn,
            Command::OverviewOff,
        ] {
            assert_eq!(cmd.to_string().parse::<Command>().unwrap(), cmd);
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

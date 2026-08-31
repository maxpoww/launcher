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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            Command::OverviewOn => f.write_str("overview-on"),
            Command::OverviewOff => f.write_str("overview-off"),
            Command::ResizeDragOn => f.write_str("resize-drag-on"),
            Command::ResizeDragOff => f.write_str("resize-drag-off"),
        }
    }
}

impl FromStr for Command {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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
            "overview-on" => Ok(Command::OverviewOn),
            "overview-off" => Ok(Command::OverviewOff),
            "resize-drag-on" => Ok(Command::ResizeDragOn),
            "resize-drag-off" => Ok(Command::ResizeDragOff),
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
            Command::OverviewOn,
            Command::OverviewOff,
        ] {
            assert_eq!(cmd.to_string().parse::<Command>().unwrap(), cmd);
        }
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

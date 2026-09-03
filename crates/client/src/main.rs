//! `waverunner-ctl` — tiny CLI that forwards a command to the daemon
//! over its Unix socket. Intended to be bound to a Hyprland keybind:
//!
//! ```text
//! bind = SUPER, SPACE, exec, waverunner-ctl toggle
//! ```

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use waverunner_proto::{Command, Response};

/// Usage text generated from the proto's own verb table
/// ([`waverunner_proto::USAGE_VERBS`]) — the hand-written string it replaces
/// had silently stopped at `debug-dict` while the protocol kept growing.
fn usage() -> String {
    format!(
        "usage: waverunner-ctl [--time] <command>\ncommands:\n  {}",
        waverunner_proto::USAGE_VERBS.join("\n  ")
    )
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let timed = args
        .iter()
        .position(|a| a == "--time")
        .map(|i| args.remove(i))
        .is_some();
    if args.is_empty() {
        bail!("{}", usage());
    }
    // Payload verbs arrive as separate argv elements from a shell
    // (`waverunner-ctl options-trigger media.playpause`); the wire format is
    // one space-joined line either way, so join rather than demand quoting.
    let cmd: Command = args.join(" ").parse().with_context(usage)?;

    let path = waverunner_proto::socket_path();
    // The daemon handles the command — including rendering and committing
    // the first frame — before it writes the response, so this round-trip
    // covers command-to-first-frame-submitted (presentation then lands on
    // the next vblank).
    let start = Instant::now();
    let mut stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "cannot connect to daemon at {} (is waverunner running?)",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    writeln!(stream, "{cmd}").context("failed to send command")?;

    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .context("failed to read daemon response")?;

    match line
        .parse::<Response>()
        .context("malformed daemon response")?
    {
        Response::Ok => {
            if timed {
                println!("round-trip: {:?}", start.elapsed());
            }
            Ok(())
        }
        Response::Err(reason) => bail!("daemon refused command: {reason}"),
    }
}

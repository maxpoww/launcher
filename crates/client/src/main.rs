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

const USAGE: &str =
    "usage: waverunner-ctl [--time] <toggle|show|hide|expand|collapse|debug-clip|debug-clip-detail|debug-notif|debug-dict>";

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let timed = args
        .iter()
        .position(|a| a == "--time")
        .map(|i| args.remove(i))
        .is_some();
    let [cmd] = args.as_slice() else {
        bail!("{USAGE}");
    };
    let cmd: Command = cmd.parse().context(USAGE)?;

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

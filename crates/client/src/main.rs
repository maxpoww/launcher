//! `waverunner-ctl` — tiny CLI that forwards a command to the daemon
//! over its Unix socket. Intended to be bound to a Hyprland keybind:
//!
//! ```text
//! bind = SUPER, SPACE, exec, waverunner-ctl toggle
//! ```

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{bail, Context};
use waverunner_proto::{Command, Response};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(cmd), None) = (args.next(), args.next()) else {
        bail!("usage: waverunner-ctl <toggle|show|hide|expand|collapse>");
    };
    let cmd: Command = cmd
        .parse()
        .context("usage: waverunner-ctl <toggle|show|hide|expand|collapse>")?;

    let path = waverunner_proto::socket_path();
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
        Response::Ok => Ok(()),
        Response::Err(reason) => bail!("daemon refused command: {reason}"),
    }
}

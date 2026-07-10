//! Unix-socket control server, integrated into the calloop event loop.
//!
//! Protocol: see `waverunner-proto`. Each connection carries exactly one
//! command line and one response line; clients are short-lived, so a
//! brief blocking read with a timeout is fine and keeps the daemon
//! single-threaded.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};
use tracing::{debug, info, warn};
use waverunner_proto::{Command, Response};

use crate::App;

/// How long the daemon will wait for a connected client to send its
/// command line before dropping the connection.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_millis(200);

/// Owns the socket file and removes it on drop so a clean shutdown does
/// not leave a stale socket behind.
pub struct SocketGuard {
    path: PathBuf,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != ErrorKind::NotFound {
                warn!("failed to remove socket {}: {e}", self.path.display());
            }
        }
    }
}

/// Bind the control socket at `path` and register it with the event loop.
///
/// A pre-existing socket file is probed with a `connect()` first: if a
/// daemon answers, we refuse to start; only a refused connection marks
/// the file as stale (previous crash) and safe to remove.
pub fn listen(handle: &LoopHandle<'static, App>, path: &Path) -> anyhow::Result<SocketGuard> {
    match UnixStream::connect(path) {
        Ok(_) => {
            return Err(anyhow!(
                "another waverunner daemon is already running on {}",
                path.display()
            ))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path)
                .with_context(|| format!("cannot clear stale socket {}", path.display()))?;
            info!("removed stale socket {}", path.display());
        }
        Err(e) => return Err(e).with_context(|| format!("cannot probe {}", path.display())),
    }

    let listener = UnixListener::bind(path)
        .with_context(|| format!("cannot bind control socket {}", path.display()))?;
    listener
        .set_nonblocking(true)
        .context("cannot set control socket non-blocking")?;
    info!("listening on {}", path.display());

    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, app| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => handle_client(stream, app),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) => {
                            warn!("accept failed on control socket: {e}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("failed to register ipc source: {e}"))?;

    Ok(SocketGuard {
        path: path.to_owned(),
    })
}

fn handle_client(stream: UnixStream, app: &mut App) {
    let response = match read_command(&stream) {
        Ok(command) => {
            // handle_command draws and commits the first frame before
            // returning, so this covers command-to-first-frame-submitted.
            let start = Instant::now();
            app.handle_command(command);
            debug!("ipc command {command} handled in {:?}", start.elapsed());
            Response::Ok
        }
        Err(e) => {
            warn!("bad ipc request: {e}");
            Response::Err(e.to_string())
        }
    };
    let mut stream = stream;
    if let Err(e) = writeln!(stream, "{response}") {
        debug!("client went away before response: {e}");
    }
}

fn read_command(stream: &UnixStream) -> anyhow::Result<Command> {
    stream
        .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
        .context("cannot set client read timeout")?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("client sent no command")?;
    line.parse::<Command>().map_err(Into::into)
}

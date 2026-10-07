//! Where the daemon's log goes: stderr, as ever, and a file of this
//! session's, because the daemon Hyprland starts at login has a stderr
//! nobody can read (it is not the journal). Every `waverunner-ctl debug-*`
//! verb answers through this log, so without the file the live daemon
//! could only be driven blind.

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::fmt::MakeWriter;

/// Environment override of the file; empty to have no file.
const LOG_ENV: &str = "WAVERUNNER_LOG";

/// The default: `$XDG_STATE_HOME/waverunner/daemon.log`.
fn default_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("waverunner").join("daemon.log"))
}

/// Writes every line to stderr and, when it could be opened, the file.
pub struct Tee {
    file: Option<Mutex<File>>,
}

impl Tee {
    /// Open (truncating) the session's log file; stderr-only if it cannot
    /// be. The path is printed to stderr once so a reader at a terminal
    /// knows where the rest goes.
    pub fn open() -> Self {
        let path = match std::env::var_os(LOG_ENV) {
            Some(p) if p.is_empty() => None,
            Some(p) => Some(PathBuf::from(p)),
            None => default_path(),
        };
        let file = path.and_then(|p| {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).ok()?;
            }
            let file = File::create(&p).ok()?;
            eprintln!("waverunner: logging to {}", p.display());
            Some(Mutex::new(file))
        });
        Self { file }
    }
}

/// One write handle: stderr plus the locked file.
pub struct TeeWriter<'a> {
    file: Option<&'a Mutex<File>>,
}

impl Write for TeeWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        if let Some(file) = self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.write_all(buf);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(file) = self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.flush();
            }
        }
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter {
            file: self.file.as_ref(),
        }
    }
}

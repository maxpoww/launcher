//! Detached application launch.
//!
//! Double-fork + `setsid` so launched apps belong to their own session:
//! they survive a daemon restart, and the intermediate child is reaped
//! immediately so the daemon never accumulates zombies.

use std::ffi::CString;

use anyhow::{bail, Context};
use tracing::info;

/// Launch `exec` (a shell command line, field codes already stripped)
/// fully detached from the daemon. `Terminal=true` entries run inside
/// the configured `terminal` command instead of headless.
pub fn launch(exec: &str, needs_terminal: bool, terminal: &str) -> anyhow::Result<()> {
    let line = if needs_terminal {
        format!("{terminal} sh -c {}", shell_quote(exec))
    } else {
        exec.to_owned()
    };
    info!("launching: {line}");
    let sh = CString::new("/bin/sh").context("sh path")?;
    let dash_c = CString::new("-c").context("-c arg")?;
    let cmd = CString::new(line).context("exec line contains a NUL byte")?;

    // SAFETY: standard double-fork daemonization. Between fork and exec
    // the child only calls async-signal-safe functions (setsid, fork,
    // open, dup2, execv, _exit) — no allocation, no locks.
    unsafe {
        match libc::fork() {
            -1 => bail!("fork failed: {}", std::io::Error::last_os_error()),
            0 => {
                // First child: new session, then fork again and exit so
                // the grandchild is reparented to init.
                libc::setsid();
                match libc::fork() {
                    0 => {
                        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
                        if devnull >= 0 {
                            libc::dup2(devnull, 0);
                            libc::dup2(devnull, 1);
                            libc::dup2(devnull, 2);
                            if devnull > 2 {
                                libc::close(devnull);
                            }
                        }
                        let argv = [sh.as_ptr(), dash_c.as_ptr(), cmd.as_ptr(), std::ptr::null()];
                        libc::execv(sh.as_ptr(), argv.as_ptr());
                        libc::_exit(127);
                    }
                    _ => libc::_exit(0),
                }
            }
            pid => {
                // Reap the short-lived intermediate child right away.
                let mut status = 0;
                libc::waitpid(pid, &mut status, 0);
                Ok(())
            }
        }
    }
}

/// Single-quote `s` for a POSIX shell (embedded quotes become `'\''`).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("nvim"), "'nvim'");
        assert_eq!(shell_quote("echo 'hi'"), r"'echo '\''hi'\'''");
    }
}

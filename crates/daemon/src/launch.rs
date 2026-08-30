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
    // Self-heal a stale Chrome profile lock before a webapp launch (no-op for
    // everything else) so webapps keep opening after an unclean Chrome exit or
    // a machine rename. See webapps::clear_stale_profile_lock_for.
    crate::webapps::clear_stale_profile_lock_for(exec);
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
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Shell command printing the Golem tool banner, shared by every
/// path that lands the user in a terminal with a CLI tool ready (the
/// "try it" nix shell and installed CLI tiles launched from the grid):
/// Golem in the zsh comment grey (#928374); the package name in
/// bold with the optional version and the run line in white; and "is
/// ready" in the same green zsh paints a valid command (#6abf69).
/// Colors live in the printf format; package/version/program arrive as
/// %s args so an odd char can't be read as an escape. Ends with `;` so
/// callers append the shell to `exec` into.
pub fn banner_cmd(pkg: &str, version: Option<&str>, program: &str) -> String {
    match version {
        Some(ver) => format!(
            "printf '\\n\\033[38;2;146;131;116mGolem\\033[0m\\n\
             \\033[1;97m%s\\033[0m\\033[97m version: %s - \\033[38;2;106;191;105mis ready\\033[0m\\n\
             \\033[97mrun: %s\\033[0m\\n' {} {} {};",
            shell_quote(pkg),
            shell_quote(ver),
            shell_quote(program),
        ),
        None => format!(
            "printf '\\n\\033[38;2;146;131;116mGolem\\033[0m\\n\
             \\033[1;97m%s\\033[0m\\033[97m - \\033[38;2;106;191;105mis ready\\033[0m\\n\
             \\033[97mrun: %s\\033[0m\\n' {} {};",
            shell_quote(pkg),
            shell_quote(program),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("nvim"), "'nvim'");
        assert_eq!(shell_quote("echo 'hi'"), r"'echo '\''hi'\'''");
    }

    #[test]
    fn banner_quotes_args_and_matches_placeholders() {
        let with = banner_cmd("ripgrep", Some("14.1"), "rg");
        // Three %s placeholders, three quoted args, trailing `;`.
        assert_eq!(with.matches("%s").count(), 3);
        assert!(with.contains("'ripgrep'") && with.contains("'14.1'") && with.contains("'rg'"));
        assert!(with.trim_end().ends_with(';'));
        let without = banner_cmd("fastfetch", None, "fastfetch");
        assert_eq!(without.matches("%s").count(), 2);
        assert!(!without.contains("version"));
    }
}

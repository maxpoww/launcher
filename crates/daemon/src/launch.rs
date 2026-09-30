//! Detached application launch.
//!
//! Double-fork + `setsid` so launched apps belong to their own session:
//! they survive a daemon restart, and the intermediate child is reaped
//! immediately so the daemon never accumulates zombies.

use std::ffi::CString;
use std::sync::OnceLock;

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
    warn_if_unresolvable(&line);
    let line = scoped(&line, exec, scopes_available());
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

/// Run the launch in its own systemd user scope (2026-09-26). Every app the
/// dock started used to inherit the compositor's cgroup, which has no CPU
/// controller — so nothing in the session could weigh one process against
/// another (Beam wanted its background tabs below the foreground one). A
/// scope per app, under app-graphical.slice (uwsm's convention, so the session
/// tears them down with it), with `CPUWeight=` set so systemd enables the cpu
/// controller on the path and `Delegate=yes` so the app owns its subtree.
/// `systemd-run --scope` execs the command in-process, so the double-fork and
/// the PID the compositor sees are unchanged. Plain launch when scopes aren't
/// available (probed once, see `scopes_available`).
fn scoped(line: &str, exec: &str, available: bool) -> String {
    if !available {
        return line.to_owned();
    }
    let slug: String = exec
        .split_whitespace()
        .find(|t| !t.contains('='))
        .unwrap_or("app")
        .rsplit('/')
        .next()
        .unwrap_or("app")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    let slug = if slug.is_empty() { "app".to_owned() } else { slug.to_ascii_lowercase() };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "systemd-run --user --scope --quiet --collect --slice=app-graphical.slice \
         -p CPUWeight=100 -p Delegate=yes --unit=golem-app-{slug}-{nonce} -- /bin/sh -c {}",
        shell_quote(line)
    )
}

/// Whether a transient user scope can be created here — probed ONCE (a ~20ms
/// `systemd-run … true`), so a session without a reachable user manager (no
/// session bus, a bare test harness) keeps launching apps the plain way instead
/// of failing every launch.
fn scopes_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        if !on_path("systemd-run") {
            return false;
        }
        let ok = std::process::Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", "-p", "CPUWeight=100", "--", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        info!("app scopes (systemd-run --user --scope): {}", if ok { "available" } else { "unavailable, plain launches" });
        ok
    })
}

/// Single-quote `s` for a POSIX shell (embedded quotes become `'\''`).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Whether `bin` resolves to a file on `$PATH` (or exists, if it has a slash).
pub(crate) fn on_path(bin: &str) -> bool {
    if bin.contains('/') {
        return std::path::Path::new(bin).exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Best-effort visibility for a detached launch: the grandchild's exit is
/// swallowed by design (double-fork, fds on /dev/null), so a missing binary
/// means "nothing happens" with no trace anywhere. If the command's first
/// token clearly can't resolve, say so in the log before launching anyway.
/// Skipped for lines starting with an env assignment or shell syntax — those
/// are for `sh` to judge.
fn warn_if_unresolvable(line: &str) {
    let Some(tok) = line.split_whitespace().next() else {
        return;
    };
    if tok.contains('=') || tok.starts_with(['(', '{', '!']) {
        return;
    }
    if !on_path(tok) {
        tracing::warn!("launch: '{tok}' not found on PATH — the command will exit 127 silently");
    }
}

/// Shell command printing the Golem tool banner, shared by every
/// path that lands the user in a terminal with a CLI tool ready (the
/// "try it" nix shell and installed CLI tiles launched from the grid):
/// Golem in the zsh comment grey (#928374); the package name in
/// bold with the optional version and the run line in white; and "is
/// ready" in the same green zsh paints a valid command (#6abf69).
/// Colors live in the printf format; package/version/program — and the
/// banner's translatable words — arrive as %s args so an odd char (or a
/// quote in a translation) can't be read as an escape or break the
/// single-quoted format. Ends with `;` so callers append the shell to
/// `exec` into.
pub fn banner_cmd(pkg: &str, version: Option<&str>, program: &str) -> String {
    match version {
        Some(ver) => format!(
            "printf '\\n\\033[38;2;146;131;116mGolem\\033[0m\\n\
             \\033[1;97m%s\\033[0m\\033[97m %s %s - \\033[38;2;106;191;105m%s\\033[0m\\n\
             \\033[97m%s %s\\033[0m\\n' {} {} {} {} {} {};",
            shell_quote(pkg),
            shell_quote(crate::i18n::tr("version:")),
            shell_quote(ver),
            shell_quote(crate::i18n::tr("is ready")),
            shell_quote(crate::i18n::tr("run:")),
            shell_quote(program),
        ),
        None => format!(
            "printf '\\n\\033[38;2;146;131;116mGolem\\033[0m\\n\
             \\033[1;97m%s\\033[0m\\033[97m - \\033[38;2;106;191;105m%s\\033[0m\\n\
             \\033[97m%s %s\\033[0m\\n' {} {} {} {};",
            shell_quote(pkg),
            shell_quote(crate::i18n::tr("is ready")),
            shell_quote(crate::i18n::tr("run:")),
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
    fn scoped_wraps_in_a_named_user_scope() {
        let s = scoped("firefox -P golem", "firefox -P golem", true);
        assert!(s.starts_with("systemd-run --user --scope --quiet --collect --slice=app-graphical.slice"));
        assert!(s.contains("-p CPUWeight=100 -p Delegate=yes --unit=golem-app-firefox-"));
        assert!(s.ends_with("-- /bin/sh -c 'firefox -P golem'"));
        // env assignments and paths don't leak into the unit name; quotes survive
        let s = scoped("FOO=1 /usr/bin/x-term -e 'a b'", "FOO=1 /usr/bin/x-term -e 'a b'", true);
        assert!(s.contains("--unit=golem-app-xterm-"));
        assert!(s.ends_with(&format!("-- /bin/sh -c {}", shell_quote("FOO=1 /usr/bin/x-term -e 'a b'"))));
        // unavailable → untouched
        assert_eq!(scoped("firefox", "firefox", false), "firefox");
    }

    #[test]
    fn on_path_resolves_absolute_and_searches_path() {
        // Absolute paths short-circuit the PATH walk.
        assert!(on_path("/bin/sh"));
        assert!(!on_path("/definitely/not/a/binary"));
        // `sh` exists on any PATH these tests run under; gibberish doesn't.
        assert!(on_path("sh"));
        assert!(!on_path("waverunner-no-such-binary-xyzzy"));
    }

    #[test]
    fn banner_quotes_args_and_matches_placeholders() {
        let with = banner_cmd("ripgrep", Some("14.1"), "rg");
        // Six %s placeholders (pkg, "version:", ver, "is ready", "run:",
        // program), quoted args, trailing `;`.
        assert_eq!(with.matches("%s").count(), 6);
        assert!(with.contains("'ripgrep'") && with.contains("'14.1'") && with.contains("'rg'"));
        assert!(with.trim_end().ends_with(';'));
        let without = banner_cmd("fastfetch", None, "fastfetch");
        assert_eq!(without.matches("%s").count(), 4);
        assert!(!without.contains("version"));
    }
}

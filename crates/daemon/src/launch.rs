//! Detached application launch.
//!
//! Double-fork + `setsid` so launched apps belong to their own session:
//! they survive a daemon restart, and the intermediate child is reaped
//! immediately so the daemon never accumulates zombies.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

use anyhow::{bail, Context};
use tracing::info;

/// Hyprland's "open this where it was started" ticket.
///
/// Every process the compositor execs gets `HL_INITIAL_WORKSPACE_TOKEN=<id>`
/// in its environment, bound to the workspace that was showing at that
/// moment; the first WINDOW to open from a process carrying it is sent to
/// that workspace, silently, and the ticket is spent
/// (`misc:initial_workspace_tracking = 1`). For an app started from a keybind
/// that is the point of it. For THIS process it is a trap: the compositor
/// execs the daemon too, the daemon never opens a window (its surfaces are
/// layers), so its ticket is never spent — and every app it launches inherits
/// the ticket. The first one to open a window went to wherever the DAEMON had
/// been started: workspace 1 after a login, or whatever was showing at its
/// last restart (Max, 2026-10-13: apps from the dock and fast launch *"se
/// abren en el workspace en el que estaba antes"*). After that one the ticket
/// was spent and launches behaved, until the next restart armed another.
///
/// So the ticket is dropped twice over: from the daemon's own environment as
/// it starts (`drop_workspace_ticket`), and from the environment handed to
/// every launch (`launch_env`) — which holds even if something puts one back.
/// An app the dock starts opens on the workspace you are on when it appears.
pub const WORKSPACE_TICKET: &str = "HL_INITIAL_WORKSPACE_TOKEN";

/// Drop the compositor's ticket from this process (see [`WORKSPACE_TICKET`]).
/// Call it first thing in `main`, before any thread exists. Returns whether
/// there was one, for the log.
pub fn drop_workspace_ticket() -> bool {
    let had = std::env::var_os(WORKSPACE_TICKET).is_some();
    std::env::remove_var(WORKSPACE_TICKET);
    had
}

/// The environment a launched app gets: this process's, less the ticket,
/// with the language and the time zone chosen in the control panel in place
/// of the session's (an app started now follows a choice made a moment ago;
/// the rest of the session follows at the next login).
/// Built in the parent — nothing may allocate between `fork` and `exec`.
fn launch_env() -> Vec<CString> {
    let chosen = crate::controls::session_vars(&crate::display::GolemSettings::load());
    env_with(std::env::vars_os(), &chosen)
}

/// `vars` less the ticket, with `chosen` put in place of their namesakes.
fn env_with(vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>, chosen: &[(&str, String)]) -> Vec<CString> {
    let mut env: Vec<CString> = vars
        .filter(|(k, _)| k != WORKSPACE_TICKET && !chosen.iter().any(|(c, _)| k.to_str() == Some(*c)))
        .filter_map(|(k, v)| {
            let mut kv = k.as_bytes().to_vec();
            kv.push(b'=');
            kv.extend_from_slice(v.as_bytes());
            CString::new(kv).ok()
        })
        .collect();
    env.extend(chosen.iter().filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok()));
    env
}

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
    let env = launch_env();
    let mut envp: Vec<*const libc::c_char> = env.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());

    // SAFETY: standard double-fork daemonization. Between fork and exec
    // the child only calls async-signal-safe functions (setsid, fork,
    // open, dup2, execve, _exit) — no allocation, no locks. (`envp` and the
    // strings it points into were built above, in the parent.)
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
                        libc::execve(sh.as_ptr(), argv.as_ptr(), envp.as_ptr());
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
    fn a_launch_never_carries_the_compositors_workspace_ticket() {
        // One test owns the variable (tests share the process environment).
        std::env::set_var(WORKSPACE_TICKET, "d0c5-stale");
        let has = |env: &[CString]| env.iter().any(|e| e.to_bytes().starts_with(b"HL_INITIAL_WORKSPACE_TOKEN="));
        // Even with one in our own environment, the launch's has none…
        assert!(!has(&launch_env()));
        // …and the rest of the environment is passed on.
        assert!(launch_env().iter().any(|e| e.to_bytes().starts_with(b"PATH=")));
        // The daemon drops its own at start, and says whether it had one.
        assert!(drop_workspace_ticket());
        assert!(std::env::var_os(WORKSPACE_TICKET).is_none());
        assert!(!drop_workspace_ticket());
        assert!(!has(&launch_env()));
    }

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("nvim"), "'nvim'");
        assert_eq!(shell_quote("echo 'hi'"), r"'echo '\''hi'\'''");
    }

    #[test]
    fn a_launch_carries_the_chosen_language_and_zone() {
        let vars = [("LANG", "en_US.UTF-8"), ("HOME", "/home/x"), (WORKSPACE_TICKET, "t")]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()));
        let env = env_with(vars, &[("LANG", "es_BO.UTF-8".to_owned()), ("TZ", "America/La_Paz".to_owned())]);
        let text: Vec<&str> = env.iter().filter_map(|c| c.to_str().ok()).collect();
        assert_eq!(text, ["HOME=/home/x", "LANG=es_BO.UTF-8", "TZ=America/La_Paz"]);
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

//! Make a long-lived helper subprocess die when we do.
//!
//! The engine and the daemon both keep helper processes alive for the whole
//! session — `wl-paste --watch` for the clipboard (twice: the daemon's history
//! worker and the selection collector) and `pw-mon` for audio. Tokio's
//! `kill_on_drop` retires them on a *clean* exit, but it is ordinary userspace
//! cleanup: it cannot run when waverunner is `SIGKILL`ed, panics, or is replaced
//! mid-session by a dev restart. The helpers are then reparented to `systemd`
//! and live forever.
//!
//! That is not hypothetical. A single day of dev restarts (2026-09-13) left
//! **178 orphaned `wl-paste --watch` processes** holding ~358 MB and a Wayland
//! connection each — every one of them a client the compositor still serves.
//!
//! So the child asks the kernel to do it instead: `PR_SET_PDEATHSIG` fires the
//! moment our thread goes away, however it goes away. Belt (kernel) and braces
//! (`kill_on_drop`) — the first covers the crash, the second the tidy shutdown.
//!
//! Caveat worth knowing: `PR_SET_PDEATHSIG` tracks the *thread* that forked, not
//! the process. Every caller here spawns from a thread that lives as long as the
//! process does (the brain thread, the clipboard worker), so the distinction
//! does not bite — but a helper spawned from a short-lived task would die with
//! that task.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Arrange for `cmd`'s child to be `SIGTERM`ed when the spawning thread dies.
///
/// Call once, before `spawn()`. Safe to use on any helper we would otherwise
/// leak; harmless on one that exits promptly.
pub fn die_with_parent(cmd: &mut Command) {
    // SAFETY: `pre_exec` runs between fork and exec, where only async-signal-safe
    // calls are legal. `prctl` and `getppid` are both on that list, and neither
    // allocates nor touches the parent's memory.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            // The parent can die in the window between fork and the prctl above,
            // in which case the signal we just armed will never be sent. Having
            // already been reparented is the tell, and the whole point is not to
            // outlive them — so leave.
            if libc::getppid() == 1 {
                libc::_exit(0);
            }
            Ok(())
        });
    }
}

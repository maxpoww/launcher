//! Don't let the shell's own screen capture read as "someone is watching you".
//!
//! The topbar colour-matches itself to the window beneath it by sampling that
//! window's top row over `wlr-screencopy` (see the daemon's `screencopy`
//! module), on a ~700 ms poll. Hyprland announces EVERY screencopy session on
//! its event socket as `screencast>>1,monitor` … `screencast>>0,monitor` — it
//! cannot tell our own bar apart from OBS. So the compositor collector saw a
//! screen share start and stop about once a second, and the Mind faithfully
//! surfaced its "Screen is being shared" privacy warning, which blinked on and
//! off forever (caught on the 2013 Air, 2026-09-02: 18 toggles in 8 s).
//!
//! A warning that cries wolf twice a second is worse than no warning: it trains
//! the eye to ignore the one signal that must never be ignored. So the daemon
//! declares its own captures here, and the collector drops compositor
//! screencast events that fall inside one.
//!
//! The window stays open for a [`GRACE`] after the capture finishes, because
//! the compositor's announcement travels a different path (the event socket)
//! than the capture itself and can land slightly late.
//!
//! Deliberately NOT a debounce ("ignore shares shorter than N"): a real share
//! that starts and stops quickly is exactly when the user most needs to know.
//! This suppresses only what we know to be ours.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// How long after our capture ends its announcement is still treated as ours.
/// Generous next to the ~700 ms poll: a late event must not outlive the window,
/// and a real share announced in this sliver is announced again by its own
/// state anyway (the compositor holds `screencast` on for its whole session).
const GRACE_MS: u64 = 400;

/// Captures currently in flight (normally 0 or 1; the counter tolerates
/// overlap without an early `end` reopening the gate).
static IN_FLIGHT: AtomicI64 = AtomicI64::new(0);
/// Milliseconds (since [`epoch`]) until which a just-finished capture still
/// owns the compositor's screencast announcements.
static GRACE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

/// Process start, so the two atomics can hold a plain millisecond counter.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// Mark the start of one of our own screen captures.
pub fn begin_self_capture() {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    // Keep the grace ahead of us for as long as the capture runs, so a capture
    // that never reports back (a failed frame the compositor already announced)
    // still can't leave the gate open forever.
    GRACE_UNTIL_MS.store(now_ms() + GRACE_MS, Ordering::SeqCst);
}

/// Mark the end of one of our own captures (ready, failed, or aborted). Safe to
/// call more than once for the same capture — the counter never goes negative.
pub fn end_self_capture() {
    let prev = IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    if prev <= 0 {
        IN_FLIGHT.store(0, Ordering::SeqCst);
    }
    GRACE_UNTIL_MS.store(now_ms() + GRACE_MS, Ordering::SeqCst);
}

/// Whether a compositor screencast announcement arriving right now is ours.
pub fn self_capture_active() -> bool {
    IN_FLIGHT.load(Ordering::SeqCst) > 0 || now_ms() < GRACE_UNTIL_MS.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate is closed only while one of our captures is in flight (or in
    /// its grace), and an unbalanced `end` can never take it negative.
    #[test]
    fn guard_opens_and_closes_around_our_capture() {
        // Serialised by construction: this is the only test touching the
        // statics, and it leaves them back at rest.
        assert!(!self_capture_active() || now_ms() < GRACE_UNTIL_MS.load(Ordering::SeqCst));
        begin_self_capture();
        assert!(self_capture_active(), "ours while in flight");
        begin_self_capture();
        end_self_capture();
        assert!(self_capture_active(), "still ours: one capture remains");
        end_self_capture();
        assert_eq!(IN_FLIGHT.load(Ordering::SeqCst), 0);
        // The grace keeps it ours for a moment after the last capture ends.
        assert!(self_capture_active(), "grace still holds");
        end_self_capture();
        assert_eq!(IN_FLIGHT.load(Ordering::SeqCst), 0, "never negative");
        // Past the grace it opens again (simulated by expiring it).
        GRACE_UNTIL_MS.store(0, Ordering::SeqCst);
        assert!(!self_capture_active(), "open once the grace expires");
    }
}

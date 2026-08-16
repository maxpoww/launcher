//! Shared notification types for the frontend.
//!
//! The wire type is defined once in `options-notify` (the daemon must emit it),
//! so both processes share a single schema — we re-export it here rather than
//! redeclaring it. On top of it sit the small enums that cross the boundary
//! between the D-Bus worker thread and the calloop UI thread.

pub use options_notify::ActiveNotification;

/// An update flowing **from** the D-Bus worker **to** the UI (calloop) thread.
#[derive(Debug, Clone)]
pub enum NotifEvent {
    /// The full active list — sent once on connect (hydration) and on every
    /// `ActiveChanged` afterwards. The UI diffs it to drive entry/exit springs.
    Active(Vec<ActiveNotification>),
    /// A notification was closed on the daemon (expiry, app-initiated close, or
    /// our own action). `reason` is the FreeDesktop code (1 expired, 2 dismissed,
    /// 3 CloseNotification, 4 undefined). Lets the UI play the exit animation for
    /// a card that vanished for a reason other than a fresh `Active` snapshot.
    Closed { id: u32, reason: u32 },
    /// The worker lost the bus / daemon; the UI should clear and await the next
    /// `Active`. (Emitted before the worker retries the connection.)
    Disconnected,
}

/// A command flowing **from** the UI thread **to** the D-Bus worker.
#[derive(Debug, Clone)]
pub enum NotifCommand {
    /// Dismiss/close a card (swipe or close button).
    Dismiss(u32),
    /// Activate one of a card's actions by key (button tap).
    Invoke { id: u32, key: String },
    /// Post an "install this webapp" prompt (opening a notification whose service
    /// isn't installed as a webapp) — sent back through the daemon as a normal
    /// notification.
    InstallPrompt { service: String },
}

/// Pair a flat `[key, label, key, label, …]` action list into `(key, label)`s —
/// the inverse of how [`ActiveNotification`] flattens them for the wire. Odd
/// trailing entries (malformed) are dropped.
pub fn action_pairs(actions: &[String]) -> Vec<(&str, &str)> {
    actions
        .chunks_exact(2)
        .map(|c| (c[0].as_str(), c[1].as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_flat_actions() {
        let a = vec![
            "default".to_string(),
            "Open".to_string(),
            "archive".to_string(),
            "Archive".to_string(),
        ];
        assert_eq!(
            action_pairs(&a),
            vec![("default", "Open"), ("archive", "Archive")]
        );
    }

    #[test]
    fn drops_odd_trailing_entry() {
        let a = vec!["lonely".to_string()];
        assert!(action_pairs(&a).is_empty());
    }
}

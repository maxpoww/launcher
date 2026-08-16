//! The session-D-Bus client for the notification OPTION.
//!
//! waverunner is the UI half of a notification "server" whose other half —
//! `options-notify` — owns `org.freedesktop.Notifications` in its own process.
//! This module is the bridge: two `#[proxy]` interfaces on one connection, and
//! the handlers that dispatch UI intents back to the daemon.
//!
//! Split across two interfaces (both on `/org/freedesktop/Notifications`):
//! - [`StandardNotificationsProxy`] — the spec interface. We use its
//!   `CloseNotification` (dismiss) and its `NotificationClosed` signal.
//! - [`OptionsControlProxy`] — the private `org.options.Notifications` control
//!   interface: `ListActive` (hydrate), `InvokeAction` (act/reply), and the
//!   `ActiveChanged` signal (live re-render).
//!
//! All parsing is `?`/`match`; there is no `unwrap`/`expect`/`panic` on any
//! path. A dropped connection is logged and retried, never fatal.

use zbus::proxy;

use super::types::ActiveNotification;

/// The private control interface exposed by `options-notify` for the OPTIONS
/// frontend (see `options-notify/src/control.rs`).
#[proxy(
    interface = "org.options.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait OptionsControl {
    /// All currently active notifications — for hydrating the UI on
    /// startup/reconnect.
    async fn list_active(&self) -> zbus::Result<Vec<ActiveNotification>>;

    /// Activate a card's action by key, or submit an inline reply when `key` is
    /// `inline-reply:<text>`. The daemon fires the matching spec signal back to
    /// the originating app and clears the card.
    async fn invoke_action(&self, id: u32, key: &str) -> zbus::Result<()>;

    /// Emitted by the daemon whenever the active list changes — carries the whole
    /// current list, so the UI diffs and animates.
    #[zbus(signal)]
    fn active_changed(&self, notifications: Vec<ActiveNotification>) -> zbus::Result<()>;
}

/// The standard FreeDesktop interface — we only touch the spec-defined bits.
#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait StandardNotifications {
    /// Close/dismiss a notification (spec method; the daemon emits
    /// `NotificationClosed` with reason 3).
    async fn close_notification(&self, id: u32) -> zbus::Result<()>;

    /// Post a notification (the spec `Notify` method) — used to surface an
    /// "install this webapp" prompt back through the daemon.
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<&str>,
        hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    /// Emitted when a notification is closed (1 expired, 2 dismissed, 3 via
    /// CloseNotification, 4 undefined).
    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

// --- UI intent handlers ----------------------------------------------------
//
// Thin, self-describing wrappers so the render-surface event handlers read as
// intent, not D-Bus mechanics. Each returns the zbus error for the worker to
// log; none can panic.

/// Dismiss a card (swipe / close button).
pub async fn dismiss_notification(
    std: &StandardNotificationsProxy<'_>,
    id: u32,
) -> zbus::Result<()> {
    std.close_notification(id).await
}

/// Activate a card's action (button tap).
pub async fn trigger_action(
    ctl: &OptionsControlProxy<'_>,
    id: u32,
    action_key: &str,
) -> zbus::Result<()> {
    ctl.invoke_action(id, action_key).await
}

/// Post an "install this webapp" prompt as a normal notification, so opening a
/// notification whose service isn't installed as a webapp guides the user rather
/// than dumping them in the browser.
pub async fn post_install_prompt(
    std: &StandardNotificationsProxy<'_>,
    service: &str,
) -> zbus::Result<()> {
    // Transient: a guidance prompt should flash and not linger in history.
    let mut hints = std::collections::HashMap::new();
    hints.insert("transient", zbus::zvariant::Value::Bool(true));
    std.notify(
        "OPTIONS",
        0,
        "system-software-install",
        &format!("Install {service}"),
        &format!("Add {service} as a webapp so its notifications open here."),
        Vec::new(),
        hints,
        8000,
    )
    .await
    .map(|_| ())
}

//! `org.options.Notifications` — the private control interface for the OPTIONS
//! frontend (waverunner).
//!
//! Kept deliberately *separate* from `org.freedesktop.Notifications`: that
//! standard interface stays spec-pure so third-party clients (`notify-send`,
//! KDE Connect, dunstctl) introspect and proxy it cleanly. Everything the UI
//! needs that the FreeDesktop spec doesn't define lives here instead:
//!
//! - [`OptionsControl::list_active`] — hydrate the UI on startup/reconnect.
//! - [`OptionsControl::active_changed`] (signal) — push the whole active list on
//!   every change, so the surface re-renders / animates without polling.
//! - [`OptionsControl::invoke_action`] — the UI acted on a card: fire the real
//!   FDN signal (`ActionInvoked`) back to the originating app and clear the card.
//!
//! Both interfaces are served on the *same* object path, so the UI reaches them
//! over one connection; `invoke_action` reaches into the standard interface via
//! the object server to emit the spec signals and mutate the shared store.

use tokio::sync::watch;
use zbus::object_server::{ObjectServer, SignalEmitter};

use crate::server::{Notifications, NotificationsSignals};
use crate::state::{ActiveNotification, NotificationEvent};

/// `NotificationClosed` reason: the user dismissed it (via the UI acting on it).
const CLOSE_DISMISSED: u32 = 2;

/// The control interface state: a read-only view of the live notification list
/// (the standard interface owns the authoritative store and the writes).
pub(crate) struct OptionsControl {
    rx: watch::Receiver<Vec<NotificationEvent>>,
}

impl OptionsControl {
    pub(crate) fn new(rx: watch::Receiver<Vec<NotificationEvent>>) -> Self {
        Self { rx }
    }

    /// Map the current store into the render-facing wire type.
    pub(crate) fn snapshot(rx: &watch::Receiver<Vec<NotificationEvent>>) -> Vec<ActiveNotification> {
        rx.borrow().iter().map(ActiveNotification::from).collect()
    }
}

#[zbus::interface(name = "org.options.Notifications")]
impl OptionsControl {
    /// The current active notifications — for the UI to hydrate its state on
    /// startup or after a reconnect.
    async fn list_active(&self) -> Vec<ActiveNotification> {
        Self::snapshot(&self.rx)
    }

    /// The UI activated a card's action. Fires `ActionInvoked` back to the
    /// originating app on the standard interface, then clears the notification
    /// (removing it from the store and emitting `NotificationClosed` with reason 2
    /// = dismissed). No-op if the id is already gone.
    async fn invoke_action(
        &self,
        id: u32,
        action_key: String,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> zbus::fdo::Result<()> {
        // Reach the standard interface to emit spec signals + mutate the store.
        let iface = server
            .interface::<_, Notifications>(crate::NOTIFY_PATH)
            .await?;
        let emitter = iface.signal_emitter();
        emitter.action_invoked(id, &action_key).await?;
        if iface.get_mut().await.remove(id) {
            emitter.notification_closed(id, CLOSE_DISMISSED).await?;
        }
        Ok(())
    }

    /// Emitted whenever the active list changes (post / replace / close /
    /// expire): carries the whole current list so the UI can diff and animate.
    #[zbus(signal)]
    async fn active_changed(
        emitter: &SignalEmitter<'_>,
        notifications: Vec<ActiveNotification>,
    ) -> zbus::Result<()>;
}


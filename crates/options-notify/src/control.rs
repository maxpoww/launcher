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
//!   FDN signal back to the originating app (`ActionInvoked`, or
//!   `NotificationReplied` for an `inline-reply:` payload) and clear the card.
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

    /// The UI activated a card's action (or submitted an inline reply). Routes
    /// to the correct FDN signal on the standard interface, then clears the
    /// notification (removing it from the store and emitting `NotificationClosed`
    /// with reason 2 = dismissed). No-op if the id is already gone.
    ///
    /// An `action_key` of `inline-reply:<text>` is treated as an inline reply:
    /// the text after the prefix is sent back via `NotificationReplied`. This
    /// keeps the UI's action path single-method while still driving the two
    /// distinct spec signals.
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

        match classify_action(&action_key) {
            Action::Reply(text) => emitter.notification_replied(id, text).await?,
            Action::Invoke(key) => emitter.action_invoked(id, key).await?,
        }

        // Clear the card only if it was actually present; tell the app it closed.
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

/// What an `InvokeAction` payload means.
enum Action<'a> {
    /// An inline reply carrying the user's text (the `inline-reply:` prefix).
    Reply(&'a str),
    /// A plain action, carrying its key.
    Invoke(&'a str),
}

/// Classify an `InvokeAction` payload: `inline-reply:<text>` is a reply carrying
/// `<text>`; anything else is a plain action key. Pure, so it's unit-testable.
fn classify_action(action_key: &str) -> Action<'_> {
    match action_key.strip_prefix("inline-reply:") {
        Some(text) => Action::Reply(text),
        None => Action::Invoke(action_key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_reply_prefix_is_a_reply_with_the_text() {
        match classify_action("inline-reply:Hello there") {
            Action::Reply(text) => assert_eq!(text, "Hello there"),
            Action::Invoke(_) => panic!("should be a reply"),
        }
    }

    #[test]
    fn empty_inline_reply_is_still_a_reply() {
        match classify_action("inline-reply:") {
            Action::Reply(text) => assert_eq!(text, ""),
            Action::Invoke(_) => panic!("should be a reply"),
        }
    }

    #[test]
    fn plain_key_is_an_action() {
        match classify_action("archive") {
            Action::Invoke(key) => assert_eq!(key, "archive"),
            Action::Reply(_) => panic!("should be an action"),
        }
    }

    #[test]
    fn prefix_only_at_the_start_counts() {
        // A key that merely contains the substring is a normal action.
        match classify_action("do-inline-reply:x") {
            Action::Invoke(key) => assert_eq!(key, "do-inline-reply:x"),
            Action::Reply(_) => panic!("only a leading prefix is a reply"),
        }
    }
}

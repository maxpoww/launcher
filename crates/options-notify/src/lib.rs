//! # options-notify — OPTIONS' own notification backend
//!
//! A complete `org.freedesktop.Notifications` D-Bus server (via `zbus`) that
//! replaces external daemons like mako/dunst. Notifications are received here,
//! parsed into the rich [`NotificationEvent`] model, and published as a live
//! list — so the notification OPTION (a vertical-list box in waverunner) renders
//! them directly, and the [`options-engine`] context can fold them in.
//!
//! Because a D-Bus name has a single owner, **any other notification daemon
//! (mako) must not be running** for [`NotificationService::start`] to claim the
//! name.
//!
//! ```no_run
//! # async fn demo() -> zbus::Result<()> {
//! let notify = options_notify::NotificationService::start().await?;
//! let mut rx = notify.subscribe();
//! loop {
//!     rx.changed().await.ok();
//!     for n in rx.borrow().iter() {
//!         println!("[{}] {} — {}", n.app_name, n.summary, n.body);
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! [`options-engine`]: https://docs.rs/options-engine

mod control;
mod server;
mod state;

pub use state::{ActiveNotification, NotificationAction, NotificationEvent, UrgencyLevel};

use tokio::sync::watch;
use zbus::object_server::{InterfaceRef, SignalEmitter};

// The `#[interface]` macro generates these traits, which carry the signal-emit
// methods (`action_invoked`, `notification_closed`, `active_changed`, …) on
// `SignalEmitter`.
use control::{OptionsControl, OptionsControlSignals};
use server::{Notifications, NotificationsSignals};

/// The well-known bus name and object path of the FreeDesktop notification
/// service.
pub const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
pub const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";

/// A running notification server. Holds the D-Bus connection (dropping it gives
/// up the name) and exposes the live notification list.
pub struct NotificationService {
    conn: zbus::Connection,
    rx: watch::Receiver<Vec<NotificationEvent>>,
}

impl NotificationService {
    /// Claim `org.freedesktop.Notifications` and start serving. Fails if another
    /// daemon already owns the name (stop mako first) or the session bus is
    /// unreachable.
    pub async fn start() -> zbus::Result<Self> {
        let (tx, rx) = watch::channel(Vec::new());
        let conn = zbus::connection::Builder::session()?
            .serve_at(NOTIFY_PATH, Notifications::new(tx))?
            // The private control interface for the OPTIONS frontend, on the
            // same object path (see `control.rs`). It reads the same live list.
            .serve_at(NOTIFY_PATH, OptionsControl::new(rx.clone()))?
            .name(NOTIFY_NAME)?
            .build()
            .await?;
        tracing::info!("options-notify: serving {NOTIFY_NAME}");

        // Bridge the internal watch to the `ActiveChanged` D-Bus signal, so the
        // frontend re-renders on every change without polling. One task, its own
        // emitter; it ends quietly when the service (and the watch) is dropped.
        spawn_active_changed(conn.clone(), rx.clone());

        Ok(Self { conn, rx })
    }

    /// Subscribe to the live list of active notifications (newest appended).
    pub fn subscribe(&self) -> watch::Receiver<Vec<NotificationEvent>> {
        self.rx.clone()
    }

    /// A snapshot of the current active notifications.
    pub fn current(&self) -> Vec<NotificationEvent> {
        self.rx.borrow().clone()
    }

    /// UI acted on a notification's button: notify the originating app
    /// (`ActionInvoked`) and clear the notification (`NotificationClosed`,
    /// dismissed).
    pub async fn invoke_action(&self, id: u32, action_key: &str) -> zbus::Result<()> {
        let iface = self.iface().await?;
        iface.signal_emitter().action_invoked(id, action_key).await?;
        self.close(&iface, id, CLOSE_DISMISSED).await
    }

    /// UI submitted an inline reply: send the text back (`NotificationReplied`)
    /// and clear the notification.
    pub async fn reply(&self, id: u32, text: &str) -> zbus::Result<()> {
        let iface = self.iface().await?;
        iface.signal_emitter().notification_replied(id, text).await?;
        self.close(&iface, id, CLOSE_DISMISSED).await
    }

    /// UI dismissed a notification (swipe/close) — clear it and tell the app.
    pub async fn dismiss(&self, id: u32) -> zbus::Result<()> {
        let iface = self.iface().await?;
        self.close(&iface, id, CLOSE_DISMISSED).await
    }

    async fn iface(&self) -> zbus::Result<InterfaceRef<Notifications>> {
        self.conn
            .object_server()
            .interface::<_, Notifications>(NOTIFY_PATH)
            .await
    }

    /// Remove from the store and emit `NotificationClosed` with `reason`
    /// (2 = dismissed, 3 = call). No-op if it's already gone.
    async fn close(
        &self,
        iface: &InterfaceRef<Notifications>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()> {
        let removed = iface.get_mut().await.remove(id);
        if removed {
            iface.signal_emitter().notification_closed(id, reason).await?;
        }
        Ok(())
    }
}

/// `NotificationClosed` reason codes (FreeDesktop): 1 expired, 2 dismissed by
/// the user, 3 closed via `CloseNotification`, 4 undefined.
const CLOSE_DISMISSED: u32 = 2;

/// Spawn the task that mirrors the live notification list onto the
/// `org.options.Notifications.ActiveChanged` signal. Fires once per store change
/// (the watch coalesces bursts to the latest), carrying the whole current list.
fn spawn_active_changed(conn: zbus::Connection, mut rx: watch::Receiver<Vec<NotificationEvent>>) {
    tokio::spawn(async move {
        let emitter = match SignalEmitter::new(&conn, NOTIFY_PATH) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("active_changed: cannot build emitter: {e}");
                return;
            }
        };
        loop {
            // Ends when the service drops the sender — clean shutdown.
            if rx.changed().await.is_err() {
                break;
            }
            let list = OptionsControl::snapshot(&rx);
            if let Err(e) = emitter.active_changed(list).await {
                tracing::warn!("active_changed: emit failed: {e}");
            }
        }
    });
}

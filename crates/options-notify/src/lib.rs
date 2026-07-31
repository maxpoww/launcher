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

mod server;
mod state;

pub use state::{NotificationAction, NotificationEvent, UrgencyLevel};

use tokio::sync::watch;

/// The well-known bus name and object path of the FreeDesktop notification
/// service.
pub const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
pub const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";

/// A running notification server. Holds the D-Bus connection (dropping it gives
/// up the name) and exposes the live notification list.
pub struct NotificationService {
    _conn: zbus::Connection,
    rx: watch::Receiver<Vec<NotificationEvent>>,
}

impl NotificationService {
    /// Claim `org.freedesktop.Notifications` and start serving. Fails if another
    /// daemon already owns the name (stop mako first) or the session bus is
    /// unreachable.
    pub async fn start() -> zbus::Result<Self> {
        let (tx, rx) = watch::channel(Vec::new());
        let conn = zbus::connection::Builder::session()?
            .serve_at(NOTIFY_PATH, server::Notifications::new(tx))?
            .name(NOTIFY_NAME)?
            .build()
            .await?;
        tracing::info!("options-notify: serving {NOTIFY_NAME}");
        Ok(Self { _conn: conn, rx })
    }

    /// Subscribe to the live list of active notifications (newest appended).
    pub fn subscribe(&self) -> watch::Receiver<Vec<NotificationEvent>> {
        self.rx.clone()
    }

    /// A snapshot of the current active notifications.
    pub fn current(&self) -> Vec<NotificationEvent> {
        self.rx.borrow().clone()
    }
}

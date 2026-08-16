//! The notification OPTION's data plane.
//!
//! `options-notify` runs as its own systemd user service owning
//! `org.freedesktop.Notifications`. This module is waverunner's client to it: a
//! dedicated worker thread (own single-thread Tokio runtime) that speaks session
//! D-Bus and talks back to the calloop UI loop **only** over channels — the same
//! discipline as the app indexer and the nix workers (one event loop; shared
//! state stays on it).
//!
//! Wiring it into the loop is one step (done where the other worker sources are
//! registered):
//!
//! ```ignore
//! let (ntx, nchan) = calloop::channel::channel();
//! let notif = notifications::spawn(ntx);         // -> NotifHandle
//! loop_handle.insert_source(nchan, |ev, _, state| state.on_notif_event(ev))?;
//! // …later, from an input handler:
//! notif.send(NotifCommand::Dismiss(id));
//! ```
//!
//! The UI thread receives [`NotifEvent`]s (hydrate / live changes / closes) and
//! sends [`NotifCommand`]s (dismiss / act / reply). The worker reconnects with
//! backoff and never brings down the loop.

pub mod dbus_client;
pub mod types;

use std::time::Duration;

use calloop::channel::Sender;
use futures_util::StreamExt;
use tokio::sync::mpsc;

// `action_pairs`/`ActiveNotification` are the render loop's entry points, re-
// exported now and consumed when the box UI lands (same pre-wiring as the mod).
#[allow(unused_imports)]
pub use types::{action_pairs, ActiveNotification, NotifCommand, NotifEvent};

use dbus_client::{OptionsControlProxy, StandardNotificationsProxy};

/// How long to wait before retrying after a connection/daemon loss.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);

/// Handle to the notification worker: send [`NotifCommand`]s to the daemon.
/// Dropping it stops the worker after its current await point.
pub struct NotifHandle {
    tx: mpsc::UnboundedSender<NotifCommand>,
}

impl NotifHandle {
    /// Queue a command for the worker. Non-blocking; safe to call from the UI
    /// thread. Silently drops if the worker has gone (logged there).
    pub fn send(&self, cmd: NotifCommand) {
        if let Err(e) = self.tx.send(cmd) {
            tracing::warn!("notification worker gone, dropping command: {e}");
        }
    }
}

/// Start the notification worker. `events` is the calloop channel the UI loop
/// listens on; the returned [`NotifHandle`] carries commands back.
pub fn spawn(events: Sender<NotifEvent>) -> NotifHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || run_worker(events, rx))
        .expect("spawn notifications worker thread");
    NotifHandle { tx }
}

/// Worker thread entry: build a single-thread runtime and run the reconnecting
/// client loop until the command channel closes (handle dropped).
fn run_worker(events: Sender<NotifEvent>, mut commands: mpsc::UnboundedReceiver<NotifCommand>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("notifications: cannot build runtime: {e}");
            return;
        }
    };

    rt.block_on(async move {
        loop {
            match session_loop(&events, &mut commands).await {
                // The command channel closed — the UI is gone; stop cleanly.
                Ok(()) => break,
                Err(e) => {
                    tracing::warn!("notifications: session ended ({e}); retrying");
                    let _ = events.send(NotifEvent::Disconnected);
                }
            }
            // Drain any commands queued while we were down would race the
            // reconnect; simplest correct thing is to wait, then rehydrate.
            tokio::time::sleep(RECONNECT_BACKOFF).await;
            if commands.is_closed() {
                break;
            }
        }
    });
}

/// One connected session: hydrate, then multiplex signals and commands until
/// the connection drops (Err) or the command channel closes (Ok).
async fn session_loop(
    events: &Sender<NotifEvent>,
    commands: &mut mpsc::UnboundedReceiver<NotifCommand>,
) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let ctl = OptionsControlProxy::new(&conn).await?;
    let std = StandardNotificationsProxy::new(&conn).await?;

    // Hydrate the UI with the current list.
    match ctl.list_active().await {
        Ok(list) => {
            let _ = events.send(NotifEvent::Active(list));
        }
        Err(e) => tracing::warn!("notifications: initial ListActive failed: {e}"),
    }

    let mut changed = ctl.receive_active_changed().await?;
    let mut closed = std.receive_notification_closed().await?;

    loop {
        tokio::select! {
            // Live list updates → re-render.
            sig = changed.next() => {
                let Some(sig) = sig else { return Ok(()); }; // stream ended = disconnected
                match sig.args() {
                    Ok(args) => { let _ = events.send(NotifEvent::Active(args.notifications)); }
                    Err(e) => tracing::warn!("notifications: bad ActiveChanged: {e}"),
                }
            }
            // A card closed elsewhere (expiry / app / our action) → exit anim.
            sig = closed.next() => {
                let Some(sig) = sig else { return Ok(()); };
                match sig.args() {
                    Ok(args) => {
                        let _ = events.send(NotifEvent::Closed { id: args.id, reason: args.reason });
                    }
                    Err(e) => tracing::warn!("notifications: bad NotificationClosed: {e}"),
                }
            }
            // UI intents → dispatch to the daemon.
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return Ok(()); }; // UI gone → stop
                dispatch(&ctl, &std, cmd).await;
            }
        }
    }
}

/// Send one UI intent to the daemon, logging (never panicking) on failure.
async fn dispatch(
    ctl: &OptionsControlProxy<'_>,
    std: &StandardNotificationsProxy<'_>,
    cmd: NotifCommand,
) {
    let res = match &cmd {
        NotifCommand::Dismiss(id) => dbus_client::dismiss_notification(std, *id).await,
        NotifCommand::Invoke { id, key } => dbus_client::trigger_action(ctl, *id, key).await,
        NotifCommand::InstallPrompt { service } => {
            dbus_client::post_install_prompt(std, service).await
        }
    };
    if let Err(e) = res {
        tracing::warn!("notifications: command {cmd:?} failed: {e}");
    }
}

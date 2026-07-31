//! The OPTIONS notification daemon.
//!
//! Claims `org.freedesktop.Notifications` and serves it for the session — the
//! system's primary notification server (run as a systemd user service; mako
//! must not be installed/running). It keeps the D-Bus name owned for the life
//! of the process; the notification OPTION surface subscribes to the live list.

use tokio::signal;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "options_notify=info".into()),
        )
        .init();

    let svc = match options_notify::NotificationService::start().await {
        Ok(svc) => svc,
        Err(e) => {
            tracing::error!(
                "could not claim {}: {e} — is another notification daemon (mako) running?",
                options_notify::NOTIFY_NAME
            );
            std::process::exit(1);
        }
    };
    tracing::info!(
        "OPTIONS notification daemon owning {}",
        options_notify::NOTIFY_NAME
    );

    // Keep the name owned until the session ends; log activity for the journal.
    let mut rx = svc.subscribe();
    loop {
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    // The server (and its D-Bus connection) went away — e.g. the
                    // session bus restarted. Exit non-zero so systemd restarts
                    // us and we reclaim the name (graceful fallback).
                    tracing::error!("notification server stopped (bus lost?); exiting for restart");
                    std::process::exit(1);
                }
                tracing::debug!(active = rx.borrow().len(), "notification list changed");
            }
            _ = signal::ctrl_c() => {
                tracing::info!("shutting down");
                return;
            }
        }
    }
}

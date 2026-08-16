//! Layer 4 (part) — the OPTIONS notification daemon's live list.
//!
//! Senses `org.options.Notifications` (the private control interface the
//! notification daemon serves alongside the standard FreeDesktop one) and folds
//! the *unread* notifications into a small [`NotificationContext`] the mind can
//! reason about — "you have a critical notification", "N unread". It does **not**
//! carry the rich list or drive interaction: the notification OPTION itself owns
//! that (over its own D-Bus client). This is only the ambient awareness so the
//! mind can surface *whether it's worth attention*.
//!
//! Polling (≈1s `ListActive`) rather than the `ActiveChanged` signal for now:
//! robust and simple, one tiny method call, deduplicated so a delta only lands
//! when the summary actually changes. Its own [`Layer::Notifications`] liveness
//! reflects the daemon being reachable, so the mind never surfaces stale
//! notification state if the daemon is down.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, Layer, NotificationContext};

const POLL: Duration = Duration::from_millis(1000);
const RECONNECT: Duration = Duration::from_secs(5);
const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const CONTROL_IFACE: &str = "org.options.Notifications";
/// FreeDesktop urgency: Critical.
const URGENCY_CRITICAL: u8 = 2;

#[derive(Default)]
pub struct NotificationCollector;

impl NotificationCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for NotificationCollector {
    fn name(&self) -> &'static str {
        "notifications"
    }
    fn layer(&self) -> Layer {
        Layer::Notifications
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            loop {
                let proxy = match connect().await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!("notifications: connect failed: {e}");
                        tokio::time::sleep(RECONNECT).await;
                        continue;
                    }
                };
                let mut last: Option<NotificationContext> = None;
                loop {
                    match list_active(&proxy).await {
                        Ok(list) => {
                            let ctx = summarize(&parse(list));
                            if last.as_ref() != Some(&ctx) {
                                last = Some(ctx.clone());
                                if tx
                                    .send(Update::Delta(
                                        Layer::Notifications,
                                        ContextDelta::Notifications(ctx),
                                    ))
                                    .await
                                    .is_err()
                                {
                                    return Ok(()); // aggregator gone
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!("notifications: ListActive failed, reconnecting: {e}");
                            break;
                        }
                    }
                    tokio::time::sleep(POLL).await;
                }
                // Lost the daemon: mark the layer dead so the mind stops trusting
                // the last summary, then reconnect.
                let _ = tx.send(Update::Health(Layer::Notifications, false)).await;
                tokio::time::sleep(RECONNECT).await;
            }
        })
    }
}

/// Connect to the session bus and build a proxy for the control interface.
async fn connect() -> zbus::Result<Proxy<'static>> {
    let conn = Connection::session().await?;
    Proxy::new(&conn, DEST, PATH, CONTROL_IFACE).await
}

/// Call `ListActive`, returning the raw `a{sv}` dicts.
async fn list_active(proxy: &Proxy<'_>) -> zbus::Result<Vec<HashMap<String, OwnedValue>>> {
    proxy.call("ListActive", &()).await
}

/// One active notification's fields the context cares about (parsed out of the
/// wire dict). Ordering by `timestamp_ms` picks the newest for the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawNotif {
    urgency: u8,
    app_name: String,
    summary: String,
    timestamp_ms: i64,
}

/// Extract the fields we need from the wire dicts, dropping any that can't be
/// read (best-effort — a malformed entry just doesn't contribute).
fn parse(list: Vec<HashMap<String, OwnedValue>>) -> Vec<RawNotif> {
    list.into_iter()
        .map(|d| RawNotif {
            urgency: as_u8(&d, "urgency").unwrap_or(1),
            app_name: as_string(&d, "app_name").unwrap_or_default(),
            summary: as_string(&d, "summary").unwrap_or_default(),
            timestamp_ms: as_i64(&d, "timestamp_ms").unwrap_or(0),
        })
        .collect()
}

/// Fold the active notifications into the context summary. Pure, so it's tested.
fn summarize(notifs: &[RawNotif]) -> NotificationContext {
    let newest = notifs.iter().max_by_key(|n| n.timestamp_ms);
    NotificationContext {
        active_count: notifs.len(),
        has_critical: notifs.iter().any(|n| n.urgency == URGENCY_CRITICAL),
        latest_app: newest.map(|n| n.app_name.clone()).unwrap_or_default(),
        latest_summary: newest.map(|n| n.summary.clone()).unwrap_or_default(),
    }
}

fn as_u8(d: &HashMap<String, OwnedValue>, key: &str) -> Option<u8> {
    u8::try_from(d.get(key)?.try_clone().ok()?).ok()
}

fn as_string(d: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    String::try_from(d.get(key)?.try_clone().ok()?).ok()
}

fn as_i64(d: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    // `timestamp_ms` rides the wire as a u64 (`t`); accept either width.
    let v = d.get(key)?.try_clone().ok()?;
    u64::try_from(&v)
        .map(|u| u as i64)
        .or_else(|_| i64::try_from(&v))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(urgency: u8, app: &str, summary: &str, ts: i64) -> RawNotif {
        RawNotif {
            urgency,
            app_name: app.into(),
            summary: summary.into(),
            timestamp_ms: ts,
        }
    }

    #[test]
    fn empty_is_the_default_context() {
        assert_eq!(summarize(&[]), NotificationContext::default());
    }

    #[test]
    fn counts_and_picks_the_newest_for_identity() {
        let c = summarize(&[
            raw(1, "Mail", "Older", 100),
            raw(1, "Chat", "Newest", 300),
            raw(1, "Cal", "Middle", 200),
        ]);
        assert_eq!(c.active_count, 3);
        assert!(!c.has_critical);
        assert_eq!(c.latest_app, "Chat");
        assert_eq!(c.latest_summary, "Newest");
    }

    #[test]
    fn any_critical_flags_the_summary() {
        let c = summarize(&[raw(1, "Mail", "hi", 1), raw(2, "Alarm", "!!", 2)]);
        assert!(c.has_critical);
        assert_eq!(c.latest_app, "Alarm");
    }
}

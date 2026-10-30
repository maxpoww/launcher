//! Being told by the bus instead of asking it.
//!
//! The media and Bluetooth layers asked every second (every 3 s for Bluetooth)
//! what had changed: on a computer where nothing was playing that was a list of
//! the session bus's names a second, and on a machine with one paused player a
//! question to that player a second, each one waking the bus, the player and
//! us, for nothing (measured at rest, 2026-10-30). Players and BlueZ both say
//! when something changes; the slow look behind this is only a net.
//!
//! ⚠ A `MessageStream` left unread for long hangs the connection (zbus's
//! rule): every wait goes through [`next`], whatever else it waits for.

use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream;
use zbus::{Connection, MatchRule, MessageStream};

/// The signals `rule` matches, if the bus will have it (none: keep asking).
pub(super) async fn listen(conn: &Connection, rule: zbus::Result<MatchRule<'static>>) -> Option<MessageStream> {
    MessageStream::for_match_rule(rule.ok()?, conn, Some(64)).await.ok()
}

/// The next signal. With no stream (or one that ended) this never comes.
pub(super) async fn next(stream: &mut Option<MessageStream>) {
    if let Some(s) = stream {
        if std::future::poll_fn(|cx| Pin::new(&mut *s).poll_next(cx)).await.is_some() {
            return;
        }
        *stream = None;
    }
    std::future::pending().await
}

/// Let a burst finish: `settle` of nothing asked, then what came meanwhile
/// thrown away (the look that follows sees all of it).
pub(super) async fn settle(streams: &mut [&mut Option<MessageStream>], settle: Duration) {
    tokio::time::sleep(settle).await;
    for stream in streams {
        while tokio::time::timeout(Duration::ZERO, next(stream)).await.is_ok() {}
    }
}

//! Layer 4 (part) — MPRIS media players over the session D-Bus.
//!
//! Any player advertising `org.mpris.MediaPlayer2.*` is a source of "what am I
//! listening to / watching." This collector reconciles the set of players on a
//! short interval and reports the one that matters (a *playing* player wins over
//! a paused one), reading title/artist/status from its MPRIS properties.
//!
//! Polling (≈1s) rather than PropertiesChanged signals for now: robust, simple,
//! and plenty for media state — it can be upgraded to signal-driven later. Bus
//! traffic is a couple of tiny property reads per second, deduplicated so a
//! delta is only emitted when the reported state actually changes.
//!
//! Health note: several collectors share `Layer::Hardware`, so this one never
//! reports the layer *dead* on a transient D-Bus hiccup (the system sampler
//! keeps the layer live); it simply retries.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zbus::zvariant::OwnedValue;
use zbus::Connection;

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, Layer, MediaState};

const POLL: Duration = Duration::from_millis(1000);
const RECONNECT: Duration = Duration::from_secs(5);
const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";

#[derive(Default)]
pub struct MediaCollector;

impl MediaCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for MediaCollector {
    fn name(&self) -> &'static str {
        "media"
    }
    fn layer(&self) -> Layer {
        Layer::Hardware
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            loop {
                let conn = match Connection::session().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("media: session bus unavailable: {e}");
                        tokio::time::sleep(RECONNECT).await;
                        continue;
                    }
                };
                let mut last: Option<Option<MediaState>> = None;
                loop {
                    match reconcile(&conn).await {
                        Ok(state) => {
                            if last.as_ref() != Some(&state) {
                                last = Some(state.clone());
                                if tx
                                    .send(Update::Delta(Layer::Hardware, ContextDelta::Media(state)))
                                    .await
                                    .is_err()
                                {
                                    return Ok(()); // aggregator gone
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!("media: reconcile failed, reconnecting: {e}");
                            break;
                        }
                    }
                    tokio::time::sleep(POLL).await;
                }
                tokio::time::sleep(RECONNECT).await;
            }
        })
    }
}

/// Read every MPRIS player and choose the one that matters.
async fn reconcile(conn: &Connection) -> zbus::Result<Option<MediaState>> {
    let dbus = DBusProxy::new(conn).await?;
    let names = dbus.list_names().await?;
    let mut candidates = Vec::new();
    for name in names {
        let name = name.as_str();
        if name.starts_with(MPRIS_PREFIX) {
            if let Some(ms) = read_player(conn, name).await {
                candidates.push(ms);
            }
        }
    }
    Ok(pick_best(candidates))
}

/// A playing player wins over a paused one; otherwise the first present.
fn pick_best(candidates: Vec<MediaState>) -> Option<MediaState> {
    candidates
        .iter()
        .find(|c| c.is_playing)
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

/// Read one player's state, or `None` if it can't be queried.
async fn read_player(conn: &Connection, bus_name: &str) -> Option<MediaState> {
    let props = PropertiesProxy::builder(conn)
        .destination(bus_name.to_owned())
        .ok()?
        .path(MPRIS_PATH)
        .ok()?
        .build()
        .await
        .ok()?;

    let player = props
        .get_all(InterfaceName::try_from(PLAYER_IFACE).ok()?)
        .await
        .ok()?;

    let status = player
        .get("PlaybackStatus")
        .and_then(as_string)
        .unwrap_or_default();
    // Skip fully stopped players — nothing to surface.
    if status == "Stopped" {
        return None;
    }
    let metadata: HashMap<String, OwnedValue> = player
        .get("Metadata")
        .and_then(|v| v.try_to_owned().ok())
        .and_then(|v| HashMap::try_from(v).ok())
        .unwrap_or_default();

    let title = metadata.get("xesam:title").and_then(as_string).unwrap_or_default();
    let artist = metadata
        .get("xesam:artist")
        .and_then(as_string_list)
        .unwrap_or_default()
        .join(", ");

    // Identity is a friendlier name than the bus suffix, but fall back to it.
    let player_name = props
        .get_all(InterfaceName::try_from(ROOT_IFACE).ok()?)
        .await
        .ok()
        .and_then(|root| root.get("Identity").and_then(as_string))
        .unwrap_or_else(|| bus_name.trim_start_matches(MPRIS_PREFIX).to_owned());

    Some(MediaState {
        player_name,
        title,
        artist,
        is_playing: status == "Playing",
    })
}

/// Best-effort `OwnedValue` → `String`.
fn as_string(v: &OwnedValue) -> Option<String> {
    String::try_from(v.try_clone().ok()?).ok()
}

/// Best-effort `OwnedValue` → `Vec<String>` (MPRIS `xesam:artist` is an array).
fn as_string_list(v: &OwnedValue) -> Option<Vec<String>> {
    Vec::<String>::try_from(v.try_clone().ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(name: &str, playing: bool) -> MediaState {
        MediaState {
            player_name: name.into(),
            title: "t".into(),
            artist: "a".into(),
            is_playing: playing,
        }
    }

    #[test]
    fn prefers_a_playing_player() {
        let best = pick_best(vec![ms("paused", false), ms("playing", true)]);
        assert_eq!(best.unwrap().player_name, "playing");
    }

    #[test]
    fn falls_back_to_first_when_none_playing() {
        let best = pick_best(vec![ms("first", false), ms("second", false)]);
        assert_eq!(best.unwrap().player_name, "first");
    }

    #[test]
    fn none_when_empty() {
        assert!(pick_best(vec![]).is_none());
    }
}

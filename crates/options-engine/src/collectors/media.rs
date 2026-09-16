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
use crate::state::{ContextState, Layer, PlaybackState, Playing, PlayingSource};

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
                let mut last: Option<Vec<Playing>> = None;
                loop {
                    match reconcile(&conn).await {
                        Ok(state) => {
                            if last.as_ref() != Some(&state) {
                                last = Some(state.clone());
                                if tx
                                    .send(Update::Delta(
                                        Layer::Hardware,
                                        ContextDelta::MprisPlayers(state),
                                    ))
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

/// Read EVERY MPRIS player. Nothing is chosen and nothing is discarded — that
/// was finding #83: `pick_best` kept one and dropped the rest, so two players
/// were indistinguishable from one, and the phone won over the desktop as often
/// as not.
async fn reconcile(conn: &Connection) -> zbus::Result<Vec<Playing>> {
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
    Ok(candidates)
}

/// Read one player's state, or `None` if it can't be queried.
async fn read_player(conn: &Connection, bus_name: &str) -> Option<Playing> {
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
    // Stopped players are KEPT now. They used to be dropped here as "nothing to
    // surface", which is a decision, and deciding is not this layer's job — it
    // is also precisely how "a video paused on ws8" became unsayable. The mind
    // filters; the collector reports (pillar 2).
    let state = match status.as_str() {
        "Playing" => PlaybackState::Playing,
        "Paused" => PlaybackState::Paused,
        _ => PlaybackState::Stopped,
    };
    let metadata: HashMap<String, OwnedValue> = player
        .get("Metadata")
        .and_then(|v| v.try_to_owned().ok())
        .and_then(|v| HashMap::try_from(v).ok())
        .unwrap_or_default();

    let title = metadata
        .get("xesam:title")
        .and_then(as_string)
        .unwrap_or_default();
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

    let us_to_secs = |us: i64| (us.max(0) as u64) / 1_000_000;
    let position_secs = player
        .get("Position")
        .and_then(as_i64)
        .map(us_to_secs)
        .unwrap_or(0);
    let length_secs = metadata
        .get("mpris:length")
        .and_then(as_i64)
        .map(us_to_secs)
        .unwrap_or(0);
    // Cover art. Kept only when it is a local file — see `Playing::art_url`.
    let art_url = metadata
        .get("mpris:artUrl")
        .and_then(as_string)
        .filter(|u| u.starts_with("file://"));

    // **The join key.** D-Bus knows which process owns a bus name, so asking it
    // turns an MPRIS player into a pid — and a pid is a window, and a window is
    // a workspace. This one call is what makes "music on Spotify on ws5"
    // expressible; without it a player is a name floating free of the screen.
    //
    // `None` is a real answer, not a failure: a phone over kdeconnect and a
    // headless mpd genuinely have no local window, and the mind must be able to
    // tell "somewhere else on this machine" from "not on this machine at all".
    let pid = DBusProxy::new(conn)
        .await
        .ok()?
        .get_connection_unix_process_id(bus_name.try_into().ok()?)
        .await
        .ok();

    Some(Playing {
        source: PlayingSource::Mpris {
            bus: bus_name.to_owned(),
        },
        app: player_name,
        title,
        artist,
        state,
        pid,
        // MPRIS has no idea where its sound is routed; the PipeWire side fills
        // this in when the two are merged by pid.
        output: None,
        position_secs,
        length_secs,
        art_url,
    })
}

/// Best-effort `OwnedValue` → `String`.
fn as_string(v: &OwnedValue) -> Option<String> {
    String::try_from(v.try_clone().ok()?).ok()
}

/// Best-effort `OwnedValue` → `i64` (MPRIS Position/length are i64 or u64 µs).
fn as_i64(v: &OwnedValue) -> Option<i64> {
    let c = v.try_clone().ok()?;
    i64::try_from(c.try_clone().ok()?)
        .ok()
        .or_else(|| u64::try_from(c).ok().map(|u| u as i64))
}

/// Best-effort `OwnedValue` → `Vec<String>` (MPRIS `xesam:artist` is an array).
fn as_string_list(v: &OwnedValue) -> Option<Vec<String>> {
    Vec::<String>::try_from(v.try_clone().ok()?).ok()
}

// No unit tests here any more, and the reason is the point of finding #83:
// everything this module used to test — `pick_best` preferring a playing
// player, falling back to the first, returning None when empty — was testing
// the *discarding*. There is no longer a choice to make: every player on the
// bus is reported, and the merge that marries them to PipeWire streams is
// tested in `engine.rs` where it lives.

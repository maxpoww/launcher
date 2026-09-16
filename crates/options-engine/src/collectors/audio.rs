//! Layer 4 (part) — audio: microphone activity, sink volume and mute.
//!
//! A live microphone is a high-value signal — it usually means a call or
//! meeting, which the mind treats as the dominant activity. Volume/mute round
//! out the audio picture.
//!
//! Implementation: poll the PipeWire CLIs (`pw-dump` for capture streams,
//! `wpctl` for the default sink) on a short interval — the same robust
//! subprocess approach as the clipboard collector, with no `libpipewire` build
//! dependency. Upgradeable to a native PipeWire client later. Privacy: this
//! only senses *that* a mic is active and the sink's level — never any audio.
//!
//! Shares `Layer::Hardware` with the system sampler, so it never marks the
//! layer dead on a missing tool; it just reports what it can.

use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{
    AudioSink, AudioState, ContextState, Layer, PlaybackState, Playing, PlayingSource,
};

const POLL: Duration = Duration::from_secs(2);
/// How long to let a burst of PipeWire events settle before reading. One user
/// action changes several objects, and each would otherwise cost its own dump.
const SETTLE: Duration = Duration::from_millis(60);
/// How long after a probe finishes its own echo is still arriving.
///
/// `pw-dump` and `wpctl` are themselves PipeWire clients: each run connects and
/// disconnects, and PipeWire announces both — so the probe manufactures the very
/// events this collector watches for. Unfiltered that is a closed loop, and it
/// ran at full speed: ~8 dumps a second forever, 30% of a core on a machine
/// nobody was touching (found 2026-09-13, by battery drain).
///
/// Generous next to the ~40 ms the two probes take, because the announcement
/// travels PipeWire's event path and the `removed:` half lands only once the
/// process is reaped. The same shape as `self_capture` for screen recording:
/// suppress what we know to be ours, and let the heartbeat cover the sliver of
/// real change that shares the window.
const SELF_ECHO: Duration = Duration::from_millis(150);
/// media.class of a recording (microphone) stream node.
const MIC_CLASS: &str = "Stream/Input/Audio";
/// media.class of an application stream sending audio OUT — the things that
/// are actually making noise, MPRIS-published or not.
const STREAM_CLASS: &str = "Stream/Output/Audio";
/// media.class of an output device (a sink).
const SINK_CLASS: &str = "Audio/Sink";

#[derive(Default)]
pub struct AudioCollector;

impl AudioCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for AudioCollector {
    fn name(&self) -> &'static str {
        "audio"
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
            let mut last: Option<AudioState> = None;
            let mut last_streams: Option<Vec<Playing>> = None;
            let mut last_sinks: Option<Vec<AudioSink>> = None;
            // A change signal from `pw-mon`, so a volume key is seen at once
            // rather than up to [`POLL`] later. `None` if the tool is missing —
            // the timer below is then the only clock, exactly as before.
            let mut changes = watch_pipewire();
            loop {
                // ONE dump, read three ways. Before finding #83 this same
                // subprocess ran every two seconds and only the mic answer was
                // kept; the output streams and the sink inventory were parsed
                // away and dropped. They were never expensive — they were free.
                let dump = read_dump().await;
                let (volume, muted) = read_sink_volume().await.unwrap_or((0, false));
                let state = AudioState {
                    is_mic_active: dump.as_deref().is_some_and(mic_active_from_dump),
                    default_sink_volume: volume,
                    is_muted: muted,
                };
                if last.as_ref() != Some(&state) {
                    last = Some(state.clone());
                    if tx
                        .send(Update::Delta(Layer::Hardware, ContextDelta::Audio(state)))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }

                if let Some(raw) = dump.as_deref() {
                    let streams = output_streams_from_dump(raw);
                    if last_streams.as_ref() != Some(&streams) {
                        last_streams = Some(streams.clone());
                        if tx
                            .send(Update::Delta(
                                Layer::Hardware,
                                ContextDelta::AudioStreams(streams),
                            ))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    let sinks = sinks_from_dump(raw, volume, muted);
                    if last_sinks.as_ref() != Some(&sinks) {
                        last_sinks = Some(sinks.clone());
                        if tx
                            .send(Update::Delta(Layer::Hardware, ContextDelta::Outputs(sinks)))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                }
                // Wait for the world to change, or for the heartbeat.
                //
                // The heartbeat stays because `pw-mon` is a signal, not a
                // guarantee: it can die, it can be absent, and a missed event
                // would otherwise freeze this layer until something else
                // happened. Event-driven when it can be, polled when it must.
                //
                // Each signal carries the instant it was seen, so the probes'
                // own echo can be told from real change and dropped — see
                // [`SELF_ECHO`]. The heartbeat deadline is absolute: echoes are
                // skipped without ever postponing it.
                let echo_until = Instant::now() + SELF_ECHO;
                let heartbeat = tokio::time::Instant::now() + POLL;
                let mut watcher_died = false;
                if let Some(rx) = changes.as_mut() {
                    loop {
                        tokio::select! {
                            got = rx.recv() => match got {
                                // The watcher died; fall back to polling and
                                // stop selecting on a dead channel.
                                None => {
                                    watcher_died = true;
                                    break;
                                }
                                // Our own probes, still echoing back. Not news.
                                Some(seen) if seen < echo_until => continue,
                                Some(_) => {
                                    // Let the burst that follows one action —
                                    // PipeWire emits several objects per change
                                    // — settle before reading, so a single key
                                    // press costs one dump rather than five.
                                    tokio::time::sleep(SETTLE).await;
                                    while rx.try_recv().is_ok() {}
                                    break;
                                }
                            },
                            () = tokio::time::sleep_until(heartbeat) => break,
                        }
                    }
                }
                if watcher_died {
                    changes = None;
                }
                if changes.is_none() {
                    tokio::time::sleep_until(heartbeat).await;
                }
            }
        })
    }
}

/// Watch PipeWire for changes, yielding the instant of each event.
///
/// `pw-mon` streams object changes for the life of the session — one process,
/// no polling — so a volume key, a mute, a new stream or a default-device
/// switch is seen the moment it happens. Its output is deliberately NOT parsed:
/// the collector already knows how to read the full truth, and it only needs to
/// be told *when*. Parsing it would mean reimplementing the state model against
/// a human-readable debug format.
///
/// *When* is carried as an [`Instant`] rather than a bare unit so the collector
/// can drop the echo of its own probes ([`SELF_ECHO`]) — the alternative would
/// be to parse the stream after all, looking for our own client names.
fn watch_pipewire() -> Option<mpsc::Receiver<Instant>> {
    let mut cmd = Command::new("pw-mon");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // Session-long helper: outlive a crash and it is never cleaned up.
    crate::child::die_with_parent(cmd.as_std_mut());
    let mut child = cmd
        .spawn()
        .map_err(|e| tracing::debug!("audio: pw-mon unavailable, polling only: {e}"))
        .ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        // The child is owned by this task, so `kill_on_drop` retires it when the
        // collector goes away.
        let _child = child;
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(_)) = lines.next_line().await {
            // A full channel already means "something changed" is pending;
            // dropping the extra is the debounce. The timestamp is the *oldest*
            // pending event, which is the conservative choice: an echo that
            // queues behind real change is read as real, never the reverse.
            if tx.try_send(Instant::now()).is_err() && tx.is_closed() {
                return;
            }
        }
    });
    Some(rx)
}

/// One `pw-dump`, shared by every question below.
async fn read_dump() -> Option<String> {
    let out = Command::new("pw-dump").output().await.ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The default sink's `(volume_percent, muted)`.
async fn read_sink_volume() -> Option<(u32, bool)> {
    let out = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .await
        .ok()?;
    parse_wpctl_volume(&String::from_utf8_lossy(&out.stdout))
}

/// True if a `pw-dump` JSON array contains a running mic capture stream.
fn mic_active_from_dump(json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    value.as_array().is_some_and(|objs| {
        objs.iter().any(|o| {
            o["info"]["state"].as_str() == Some("running")
                && o["info"]["props"]["media.class"].as_str() == Some(MIC_CLASS)
        })
    })
}

/// Every application stream currently sending audio out — **including the ones
/// MPRIS never sees**.
///
/// This is the half of finding #83 that made the engine actively wrong rather
/// than merely blind: on the dev box an `ffplay` was playing to the speakers
/// while the only MPRIS players on the bus belonged to a phone over kdeconnect.
/// A game, a browser tab, a notification chime and `paplay` are all in the same
/// position — they make noise and publish nothing.
///
/// PipeWire cannot say *what* is playing (no title, no artist: that is MPRIS's
/// job) but it can say **who** (`application.process.id` → the window),
/// **whether** (`info.state`) and **where to** (`target.object` → the sink).
/// The media collector merges its richer MPRIS view over the top by pid.
fn output_streams_from_dump(json: &str) -> Vec<Playing> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(objs) = value.as_array() else {
        return Vec::new();
    };
    objs.iter()
        .filter(|o| o["info"]["props"]["media.class"].as_str() == Some(STREAM_CLASS))
        .map(|o| {
            let props = &o["info"]["props"];
            // A stream is `running` while it feeds the sink and `suspended`/
            // `idle` when the app holds it open without producing sound — which
            // is a pause in every way the user can perceive.
            let state = match o["info"]["state"].as_str() {
                Some("running") => PlaybackState::Playing,
                Some("idle" | "suspended") => PlaybackState::Paused,
                _ => PlaybackState::Stopped,
            };
            // `application.name` is the friendly one ("Firefox"); the binary is
            // the honest fallback for apps that set no name at all.
            let app = props["application.name"]
                .as_str()
                .or_else(|| props["application.process.binary"].as_str())
                .or_else(|| props["node.name"].as_str())
                .unwrap_or_default()
                .to_owned();
            Playing {
                source: PlayingSource::Stream {
                    node: o["id"].as_u64().unwrap_or(0) as u32,
                },
                app,
                // **PipeWire has no track title and this is not one.**
                // `media.name` is the STREAM's name — Chromium calls its
                // "Playback", others "audio-stream" — so reading it as a title
                // put the word "Playback" on the bar where a song should be
                // (2026-09-12). Titles come from MPRIS or not at all; a
                // PipeWire stream knows *that* sound is happening and where it
                // goes, never what it is.
                title: String::new(),
                artist: String::new(),
                state,
                pid: props["application.process.id"]
                    .as_u64()
                    .or_else(|| props["application.process.id"].as_str()?.parse().ok())
                    .map(|p| p as u32),
                output: props["target.object"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned),
                position_secs: 0,
                length_secs: 0,
                art_url: None,
            }
        })
        .collect()
}

/// The audio output inventory. The dev box has five sinks and the engine knew
/// the volume of exactly one of them.
///
/// Per-sink volume is not in `pw-dump` in a form worth trusting (it lives in
/// `Props` params that the dump does not always carry), so only the DEFAULT
/// sink's level is filled in — from the `wpctl` read the collector already
/// does. The rest report their identity, which is what "which speakers is this
/// going to" actually needs; a per-sink level can follow if an OPTION asks.
fn sinks_from_dump(json: &str, default_volume: u32, default_muted: bool) -> Vec<AudioSink> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(objs) = value.as_array() else {
        return Vec::new();
    };
    // The default sink's node name, as PipeWire's metadata reports it.
    let default_name = objs
        .iter()
        .filter(|o| o["type"].as_str() == Some("PipeWire:Interface:Metadata"))
        .flat_map(|o| o["metadata"].as_array().into_iter().flatten())
        .find(|m| m["key"].as_str() == Some("default.audio.sink"))
        .and_then(|m| m["value"]["name"].as_str())
        .unwrap_or_default()
        .to_owned();

    objs.iter()
        .filter(|o| o["info"]["props"]["media.class"].as_str() == Some(SINK_CLASS))
        .map(|o| {
            let props = &o["info"]["props"];
            let name = props["node.name"].as_str().unwrap_or_default().to_owned();
            let is_default = !default_name.is_empty() && name == default_name;
            AudioSink {
                description: props["node.description"]
                    .as_str()
                    .unwrap_or(&name)
                    .to_owned(),
                id: o["id"].as_u64().unwrap_or(0) as u32,
                is_default,
                volume_pct: if is_default { default_volume } else { 0 },
                muted: is_default && default_muted,
                name,
            }
        })
        .collect()
}

/// Parse `wpctl get-volume` output: `Volume: 0.65` or `Volume: 0.65 [MUTED]`.
/// Volume can exceed 1.0 (boost), so the percent isn't clamped to 100.
fn parse_wpctl_volume(s: &str) -> Option<(u32, bool)> {
    let rest = s.trim().strip_prefix("Volume:")?.trim();
    let vol: f32 = rest.split_whitespace().next()?.parse().ok()?;
    let pct = (vol * 100.0).round().max(0.0) as u32;
    Some((pct, s.contains("MUTED")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_parses_with_and_without_mute() {
        assert_eq!(parse_wpctl_volume("Volume: 0.65\n"), Some((65, false)));
        assert_eq!(parse_wpctl_volume("Volume: 0.00 [MUTED]"), Some((0, true)));
        assert_eq!(parse_wpctl_volume("Volume: 1.20"), Some((120, false)));
        assert_eq!(parse_wpctl_volume("nonsense"), None);
    }

    #[test]
    fn mic_detected_only_when_a_capture_stream_runs() {
        let running = r#"[
            {"info":{"state":"running","props":{"media.class":"Stream/Input/Audio"}}}
        ]"#;
        assert!(mic_active_from_dump(running));

        let idle = r#"[
            {"info":{"state":"idle","props":{"media.class":"Stream/Input/Audio"}}},
            {"info":{"state":"running","props":{"media.class":"Stream/Output/Audio"}}}
        ]"#;
        assert!(!mic_active_from_dump(idle));
    }

    #[test]
    fn malformed_dump_is_not_active() {
        assert!(!mic_active_from_dump("not json"));
        assert!(!mic_active_from_dump("{}"));
    }
}

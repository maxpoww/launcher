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

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{
    AudioSink, AudioState, ContextState, Layer, PlaybackState, Playing, PlayingSource,
};

/// The probe interval when there is no `pw-mon` to say when (tool missing,
/// or it died): polling is then the only clock.
const POLL: Duration = Duration::from_secs(2);
/// The safety-net interval while `pw-mon` is watching.
const HEARTBEAT: Duration = Duration::from_secs(60);
/// How often a watcher that died is started again. PipeWire restarting takes
/// `pw-mon` with it, and the collector then polled every [`POLL`] for the rest
/// of the session.
const REVIVE: Duration = Duration::from_secs(30);
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
///
/// Since round 3 the window is only the FALLBACK. The stream is read far
/// enough to say whose event a block is ([`MonReader`]): a probe's own client
/// is recognised by its pid and dropped outright, and an object no probe ever
/// touches (a node, a device, a link) is real change whenever it arrives. Only
/// what cannot be told either way — another client coming or going, a line not
/// understood — is still judged by this clock.
const SELF_ECHO: Duration = Duration::from_millis(150);
/// How many of the latest probes' pids are remembered, to know their echo by.
const PROBES_KEPT: usize = 8;
/// The least time between two probes that real change sets off. A lone change
/// is read after [`SETTLE`]; change that keeps coming (a volume slider being
/// dragged, a stream starting up object by object) is read this often and no
/// more — about what the echo window allowed when it judged everything.
const BREATH: Duration = Duration::from_millis(200);
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
            let mut last_sounding: Option<Vec<u32>> = None;
            let mut last_sinks: Option<Vec<AudioSink>> = None;
            // A change signal from `pw-mon`, so a volume key is seen at once
            // rather than up to [`POLL`] later. `None` if the tool is missing —
            // the timer below is then the only clock, exactly as before.
            let mut changes = watch_pipewire();
            // When to look for a watcher again, while there is none.
            let mut revive = tokio::time::Instant::now() + REVIVE;
            // Whether the probe about to run is a confirming one (see below).
            let mut confirming = false;
            loop {
                // ONE dump, parsed once and read four ways. Before finding #83
                // this same subprocess ran every two seconds and only the mic
                // answer was kept; the output streams and the sink inventory
                // were parsed away and dropped. They were never expensive —
                // they were free.
                let mut changed = false;
                // The watcher is told the probes' pids: how it knows their
                // echo.
                let probes = changes.as_ref().map(|w| &w.probes);
                let dump = read_dump(probes).await;
                let (volume, muted) = read_sink_volume(probes).await.unwrap_or((0, false));
                let state = AudioState {
                    is_mic_active: dump.as_deref().is_some_and(mic_active),
                    default_sink_volume: volume,
                    is_muted: muted,
                };
                if last.as_ref() != Some(&state) {
                    last = Some(state.clone());
                    changed = true;
                    if tx
                        .send(Update::Delta(Layer::Hardware, ContextDelta::Audio(state)))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }

                if let Some(objs) = dump.as_deref() {
                    let streams = output_streams(objs);
                    if last_streams.as_ref() != Some(&streams) {
                        last_streams = Some(streams.clone());
                        changed = true;
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
                    // Who can be heard: the stage deck's speaker badges.
                    // (The deck used to run a `pw-dump` of its own every
                    // second for this — and each one, a PipeWire client
                    // coming and going, set off two probes here: five
                    // processes a second for as long as the stage was up.)
                    let sounding = sounding_pids(objs);
                    if last_sounding.as_ref() != Some(&sounding) {
                        last_sounding = Some(sounding.clone());
                        changed = true;
                        if tx
                            .send(Update::Delta(
                                Layer::Hardware,
                                ContextDelta::Sounding(sounding),
                            ))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    let sinks = sink_inventory(objs, volume, muted);
                    if last_sinks.as_ref() != Some(&sinks) {
                        last_sinks = Some(sinks.clone());
                        changed = true;
                        if tx
                            .send(Update::Delta(Layer::Hardware, ContextDelta::Outputs(sinks)))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                }
                // Wait for the world to change.
                //
                // With `pw-mon` alive the layer is EVENT-driven: at rest it
                // runs no probe at all. (It used to probe every two seconds
                // regardless — two processes forked, a ~100 KB dump read a
                // line at a time: 590 wakeups a second on an idle laptop,
                // night audit 2026-10-04.) A slow heartbeat stays as the net
                // under a `pw-mon` that stopped talking without dying.
                //
                // The watcher says what each event was (see [`Change`]): a
                // probe's own echo never arrives here, and a REAL one — an
                // object no probe touches — is acted on whenever it comes, so a
                // stream that starts or stops is never mistaken for echo. (It
                // could be: a chime that ended inside the window of a probe
                // was dropped, and "playing" stood until the next heartbeat.)
                //
                // What is left is the UNKNOWN: another client coming or going,
                // or a line not understood. Each carries the instant it was
                // seen and is judged by the clock, as everything used to be —
                // see [`SELF_ECHO`]. Real change can share that window, so an
                // event dropped as echo earns ONE confirming probe once the
                // window closes. A confirming probe that found nothing new
                // earns no further one (its echo is only its own), which is
                // what keeps this from being the closed loop again.
                let echo_until = Instant::now() + SELF_ECHO;
                let rested = tokio::time::Instant::now() + BREATH;
                let poll = if changes.is_some() { HEARTBEAT } else { POLL };
                let mut heartbeat = tokio::time::Instant::now() + poll;
                let may_confirm = changed || !confirming;
                let mut confirm = false;
                confirming = false;
                let mut watcher_died = false;
                if let Some(watch) = changes.as_mut() {
                    loop {
                        tokio::select! {
                            got = watch.real.recv() => {
                                if got.is_none() {
                                    watcher_died = true;
                                    break;
                                }
                                // Let the burst that follows one action —
                                // PipeWire emits several objects per change
                                // — settle before reading, so a single key
                                // press costs one dump rather than five; and
                                // let the last probe rest ([`BREATH`]).
                                tokio::time::sleep(SETTLE).await;
                                tokio::time::sleep_until(rested).await;
                                watch.drain();
                                break;
                            }
                            got = watch.unknown.recv() => match got {
                                // The watcher died; fall back to polling and
                                // stop selecting on a dead channel.
                                None => {
                                    watcher_died = true;
                                    break;
                                }
                                // Perhaps our own probes, still echoing back —
                                // or real change hiding among them.
                                Some(seen) if seen < echo_until => {
                                    confirm = may_confirm;
                                    continue;
                                }
                                Some(_) => {
                                    tokio::time::sleep(SETTLE).await;
                                    watch.drain();
                                    break;
                                }
                            },
                            () = tokio::time::sleep_until(echo_until.into()), if confirm => {
                                // The echo has passed: drop what is left of it
                                // and look once more.
                                tokio::time::sleep(SETTLE).await;
                                watch.drain();
                                confirming = true;
                                break;
                            }
                            () = tokio::time::sleep_until(heartbeat) => break,
                        }
                    }
                }
                if watcher_died {
                    // Polling from here, at once — not from the end of a
                    // heartbeat that was counting on the watcher.
                    changes = None;
                    heartbeat = tokio::time::Instant::now() + POLL;
                    revive = tokio::time::Instant::now() + REVIVE;
                }
                if changes.is_none() {
                    tokio::time::sleep_until(heartbeat).await;
                    if tokio::time::Instant::now() >= revive {
                        changes = watch_pipewire();
                        revive = tokio::time::Instant::now() + REVIVE;
                    }
                }
            }
        })
    }
}

/// What one event on PipeWire's change stream was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// One of this collector's own probes coming or going: nothing changed.
    Echo,
    /// An object no probe ever touches (a node, a device, a port, a link…)
    /// appeared, changed or went: something did change.
    Real,
    /// Could be either: another client came or went (it may have changed
    /// something `pw-mon` does not show, like the default sink), or the stream
    /// said something this reader does not understand.
    Unknown,
}

/// The pids of the latest probes, shared with the watcher.
type Probes = Arc<Mutex<VecDeque<u32>>>;

/// The change signal from `pw-mon` (see [`watch_pipewire`]).
struct Watch {
    /// A [`Change::Real`] is pending. One slot: "something changed" does not
    /// count, and a real event must never queue behind lesser ones and be
    /// dropped with them.
    real: mpsc::Receiver<()>,
    /// The instant of each [`Change::Unknown`].
    unknown: mpsc::Receiver<Instant>,
    probes: Probes,
}

impl Watch {
    /// Forget what is pending: a probe is about to read the whole truth.
    fn drain(&mut self) {
        while self.real.try_recv().is_ok() {}
        while self.unknown.try_recv().is_ok() {}
    }
}

/// Watch PipeWire for changes.
///
/// `pw-mon` streams object changes for the life of the session — one process,
/// no polling — so a volume key, a mute, a new stream or a default-device
/// switch is seen the moment it happens. Its output is deliberately NOT read
/// for WHAT changed: the collector already knows how to read the full truth,
/// and it only needs to be told *when*. Parsing it for content would mean
/// reimplementing the state model against a human-readable debug format.
///
/// It is read for WHOSE event it is ([`MonReader`]), because the probes are
/// PipeWire clients themselves and the stream reports them too
/// ([`SELF_ECHO`]).
fn watch_pipewire() -> Option<Watch> {
    let mut cmd = Command::new("pw-mon");
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    // Session-long helper: outlive a crash and it is never cleaned up.
    crate::child::die_with_parent(cmd.as_std_mut());
    let mut child = cmd
        .spawn()
        .map_err(|e| tracing::debug!("audio: pw-mon unavailable, polling only: {e}"))
        .ok()?;
    let stdout = child.stdout.take()?;
    let (real_tx, real) = mpsc::channel(1);
    let (tx, unknown) = mpsc::channel(8);
    let probes = Probes::default();
    let ours = probes.clone();
    tokio::spawn(async move {
        // The child is owned by this task, so `kill_on_drop` retires it when the
        // collector goes away.
        let _child = child;
        let mut lines = BufReader::new(stdout).lines();
        let mut reader = MonReader::default();
        while let Ok(Some(line)) = lines.next_line().await {
            let is_probe = |pid| ours.lock().is_ok_and(|p| p.contains(&pid));
            let gone = match reader.line(&line, is_probe) {
                None | Some(Change::Echo) => false,
                // A full slot already says it.
                Some(Change::Real) => real_tx.try_send(()).is_err() && real_tx.is_closed(),
                // A full channel already means "something changed" is pending;
                // dropping the extra is the debounce. The timestamp is the
                // *oldest* pending event, which is the conservative choice: an
                // echo that queues behind real change is read as real, never
                // the reverse.
                Some(Change::Unknown) => tx.try_send(Instant::now()).is_err() && tx.is_closed(),
            };
            if gone {
                return;
            }
        }
    });
    Some(Watch {
        real,
        unknown,
        probes,
    })
}

/// Reads `pw-mon`'s stream just far enough to say whose event each block is.
///
/// A block is a header at the start of a line (`added:`, `changed:`,
/// `removed:`) and tab-indented lines under it: the object's `id:` first,
/// then (not for a removal) its `type:`, and for a client its properties,
/// among them the pid PipeWire read off its socket. (PipeWire 1.6; a stream
/// that does not look like this is all [`Change::Unknown`], which is how
/// everything was judged before.)
#[derive(Default)]
struct MonReader {
    block: Block,
    /// Every object announced, by id: whether it is a client.
    is_client: HashMap<u32, bool>,
    /// The client objects that are this collector's own probes.
    echo: HashSet<u32>,
}

/// Where in a block the reader is.
#[derive(Default, Clone, Copy)]
enum Block {
    /// No header seen yet.
    #[default]
    Outside,
    /// After the header: the id comes next. `true` for a removal.
    Head(bool),
    /// An `added:`/`changed:` block, its id known: the type comes next.
    Id(u32),
    /// A client: whose, its pid will say.
    Client(u32),
    /// Judged; the rest of the block says nothing more.
    Done,
}

impl MonReader {
    /// One line of the stream. Returns the verdict on a block as soon as it
    /// is known; `is_probe` says whether a pid is one of our probes.
    fn line(&mut self, line: &str, is_probe: impl Fn(u32) -> bool) -> Option<Change> {
        let header = match line.trim_end() {
            "added:" | "changed:" => Some(false),
            "removed:" => Some(true),
            _ => None,
        };
        if let Some(removal) = header {
            // A block that ended without saying what it was.
            let unfinished = !matches!(self.block, Block::Outside | Block::Done);
            self.block = Block::Head(removal);
            return unfinished.then_some(Change::Unknown);
        }
        // Changed fields are marked `*`; everything is indented.
        let text = line.trim_start_matches(['*', ' ', '\t']);
        match self.block {
            Block::Outside => Some(Change::Unknown),
            Block::Done => None,
            Block::Head(removal) => {
                let id = text.strip_prefix("id: ")?.trim().parse().ok()?;
                if !removal {
                    self.block = Block::Id(id);
                    return None;
                }
                self.block = Block::Done;
                let was_client = self.is_client.remove(&id);
                Some(if self.echo.remove(&id) {
                    Change::Echo
                } else if was_client == Some(false) {
                    Change::Real
                } else {
                    Change::Unknown
                })
            }
            Block::Id(id) => {
                let kind = text.strip_prefix("type: ")?;
                let client = kind.starts_with("PipeWire:Interface:Client");
                self.is_client.insert(id, client);
                if !client {
                    self.block = Block::Done;
                    return Some(Change::Real);
                }
                if self.echo.contains(&id) {
                    self.block = Block::Done;
                    return Some(Change::Echo);
                }
                // Whose client: its pid says, a few lines down.
                self.block = Block::Client(id);
                None
            }
            Block::Client(id) => {
                let pid = text
                    .strip_prefix("pipewire.sec.pid = \"")?
                    .trim_end()
                    .strip_suffix('"')?
                    .parse()
                    .ok()?;
                self.block = Block::Done;
                Some(if is_probe(pid) {
                    self.echo.insert(id);
                    Change::Echo
                } else {
                    Change::Unknown
                })
            }
        }
    }
}

/// One `pw-dump`, parsed once and shared by every question below: PipeWire's
/// objects. `None` when the tool cannot be run.
async fn read_dump(probes: Option<&Probes>) -> Option<Vec<serde_json::Value>> {
    let out = probe(Command::new("pw-dump"), probes).await?;
    Some(parse_dump(&out))
}

/// Run one probe to its end and return what it printed. Its pid is noted the
/// moment it exists — before it can have reached PipeWire — so the watcher
/// knows its echo.
async fn probe(mut cmd: Command, probes: Option<&Probes>) -> Option<Vec<u8>> {
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let (Some(pid), Some(mut kept)) = (child.id(), probes.and_then(|p| p.lock().ok())) {
        if kept.len() == PROBES_KEPT {
            kept.pop_front();
        }
        kept.push_back(pid);
    }
    Some(child.wait_with_output().await.ok()?.stdout)
}

/// The objects of a `pw-dump` (a JSON array); none when it is anything else.
fn parse_dump(raw: &[u8]) -> Vec<serde_json::Value> {
    match serde_json::from_slice(raw) {
        Ok(serde_json::Value::Array(objs)) => objs,
        _ => Vec::new(),
    }
}

/// The default sink's `(volume_percent, muted)`.
async fn read_sink_volume(probes: Option<&Probes>) -> Option<(u32, bool)> {
    let mut cmd = Command::new("wpctl");
    cmd.args(["get-volume", "@DEFAULT_AUDIO_SINK@"]);
    let out = probe(cmd, probes).await?;
    parse_wpctl_volume(&String::from_utf8_lossy(&out))
}

/// True if the objects contain a running mic capture stream.
fn mic_active(objs: &[serde_json::Value]) -> bool {
    objs.iter().any(|o| {
        o["info"]["state"].as_str() == Some("running")
            && o["info"]["props"]["media.class"].as_str() == Some(MIC_CLASS)
    })
}

/// The pid a stream's props name as its owner. A number from a native client;
/// PipeWire's PulseAudio server passes on whatever the client sent, which can
/// be a string.
fn stream_pid(props: &serde_json::Value) -> Option<u32> {
    let pid = &props["application.process.id"];
    pid.as_u64()
        .or_else(|| pid.as_str()?.parse().ok())
        .map(|p| p as u32)
}

/// The processes that can be heard: every output stream that is `running` and
/// not muted (the stream's own mute — a muted stream is silence and does not
/// count). Sorted, each pid once.
fn sounding_pids(objs: &[serde_json::Value]) -> Vec<u32> {
    let mut pids: Vec<u32> = objs
        .iter()
        .filter(|o| {
            o["info"]["props"]["media.class"].as_str() == Some(STREAM_CLASS)
                && o["info"]["state"].as_str() == Some("running")
                && o["info"]["params"]["Props"][0]["mute"].as_bool() != Some(true)
        })
        .filter_map(|o| stream_pid(&o["info"]["props"]))
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
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
fn output_streams(objs: &[serde_json::Value]) -> Vec<Playing> {
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
                pid: stream_pid(props),
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
fn sink_inventory(
    objs: &[serde_json::Value],
    default_volume: u32,
    default_muted: bool,
) -> Vec<AudioSink> {
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
        assert!(mic_active(&parse_dump(running.as_bytes())));

        let idle = r#"[
            {"info":{"state":"idle","props":{"media.class":"Stream/Input/Audio"}}},
            {"info":{"state":"running","props":{"media.class":"Stream/Output/Audio"}}}
        ]"#;
        assert!(!mic_active(&parse_dump(idle.as_bytes())));
    }

    #[test]
    fn malformed_dump_is_not_active() {
        assert!(!mic_active(&parse_dump(b"not json")));
        assert!(!mic_active(&parse_dump(b"{}")));
        assert!(output_streams(&parse_dump(b"")).is_empty());
    }

    /// Feed a `pw-mon` excerpt; the verdicts, in order.
    fn verdicts(reader: &mut MonReader, text: &str, probes: &[u32]) -> Vec<Change> {
        text.lines()
            .filter_map(|l| reader.line(l, |pid| probes.contains(&pid)))
            .collect()
    }

    /// A client block as PipeWire 1.6's `pw-mon` prints it.
    fn client(id: u32, name: &str, pid: u32) -> String {
        format!(
            "added:\n\tid: {id}\n\tpermissions: rwxm-\n\ttype: PipeWire:Interface:Client (version 3)\n \tproperties:\n \t\tpipewire.protocol = \"protocol-native\"\n \t\tpipewire.sec.pid = \"{pid}\"\n \t\tapplication.name = \"{name}\"\n \t\tapplication.process.id = \"{pid}\"\n"
        )
    }

    const STREAM: &str = "added:\n\tid: 70\n\tpermissions: rwxm-\n\ttype: PipeWire:Interface:Node (version 3)\n*\tparams:\n*\t  id:3 (Spa:Enum:ParamId:EnumFormat)\n*\t  id:2 (Spa:Enum:ParamId:Props)\n \tstate: \"suspended\"\n \tproperties:\n \t\tapplication.process.id = \"900\"\n \t\tmedia.class = \"Stream/Output/Audio\"\n";
    const STREAM_CHANGED: &str = "changed:\n\tid: 70\n\tpermissions: rwxm-\n\ttype: PipeWire:Interface:Node (version 3)\n \tparams:\n \t  id:3 (Spa:Enum:ParamId:EnumFormat)\n*\tstate: \"running\"\n";

    #[test]
    fn a_probe_is_known_by_its_pid_and_dropped_coming_and_going() {
        let mut r = MonReader::default();
        let text = client(63, "pw-dump", 4242) + "removed:\n\tid: 63\n";
        assert_eq!(
            verdicts(&mut r, &text, &[4242]),
            [Change::Echo, Change::Echo]
        );
        // The id is free again: the next object to wear it is judged afresh.
        assert_eq!(
            verdicts(&mut r, &client(63, "wpctl", 77), &[4242]),
            [Change::Unknown]
        );
    }

    #[test]
    fn another_client_is_unknown_coming_and_going() {
        let mut r = MonReader::default();
        let text = client(63, "wpctl", 5000) + "removed:\n\tid: 63\n";
        assert_eq!(
            verdicts(&mut r, &text, &[4242]),
            [Change::Unknown, Change::Unknown]
        );
    }

    #[test]
    fn a_stream_is_real_arriving_changing_and_leaving() {
        let mut r = MonReader::default();
        let text = format!("{STREAM}{STREAM_CHANGED}removed:\n\tid: 70\n");
        assert_eq!(
            verdicts(&mut r, &text, &[900]),
            [Change::Real, Change::Real, Change::Real]
        );
    }

    #[test]
    fn real_change_among_the_echo_is_still_real() {
        let mut r = MonReader::default();
        let text = client(63, "pw-dump", 4242)
            + STREAM
            + "removed:\n\tid: 63\n"
            + &client(63, "wpctl", 4243)
            + STREAM_CHANGED
            + "removed:\n\tid: 63\n";
        assert_eq!(
            verdicts(&mut r, &text, &[4242, 4243]),
            [
                Change::Echo,
                Change::Real,
                Change::Echo,
                Change::Echo,
                Change::Real,
                Change::Echo
            ]
        );
    }

    #[test]
    fn what_is_not_understood_is_unknown() {
        // No header at all: every line.
        let mut r = MonReader::default();
        assert_eq!(
            verdicts(&mut r, "something else\nentirely\n", &[]),
            [Change::Unknown, Change::Unknown]
        );
        // A removal of an object never announced.
        assert_eq!(
            verdicts(&mut r, "removed:\n\tid: 9\n", &[]),
            [Change::Unknown]
        );
        // A block that never says what it is: judged when the next begins.
        let mut r = MonReader::default();
        assert_eq!(
            verdicts(&mut r, "added:\n\tsomething: new\nremoved:\n\tid: 3\n", &[]),
            [Change::Unknown, Change::Unknown]
        );
        // A client that shows no pid.
        let mut r = MonReader::default();
        let text = "added:\n\tid: 5\n\ttype: PipeWire:Interface:Client (version 3)\n \tproperties:\nadded:\n";
        assert_eq!(verdicts(&mut r, text, &[1]), [Change::Unknown]);
    }

    #[test]
    fn only_running_unmuted_output_streams_can_be_heard() {
        let dump = br#"[
            {"info":{"state":"running","props":{"media.class":"Stream/Output/Audio","application.process.id":42},
                     "params":{"Props":[{"mute":false}]}}},
            {"info":{"state":"running","props":{"media.class":"Stream/Output/Audio","application.process.id":"7"}}},
            {"info":{"state":"running","props":{"media.class":"Stream/Output/Audio","application.process.id":42}}},
            {"info":{"state":"running","props":{"media.class":"Stream/Output/Audio","application.process.id":50},
                     "params":{"Props":[{"mute":true}]}}},
            {"info":{"state":"idle","props":{"media.class":"Stream/Output/Audio","application.process.id":60}}},
            {"info":{"state":"running","props":{"media.class":"Stream/Input/Audio","application.process.id":70}}},
            {"info":{"state":"running","props":{"media.class":"Audio/Sink"}}}
        ]"#;
        assert_eq!(sounding_pids(&parse_dump(dump)), vec![7, 42]);
        assert!(sounding_pids(&parse_dump(b"[]")).is_empty());
    }
}

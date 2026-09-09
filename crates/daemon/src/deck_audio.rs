//! Which tasks are making sound — the deck's speaker indicator.
//!
//! Pure status, deliberately: a white speaker on a tile whose task is audibly
//! playing, nothing on a silent one, **no click behaviour**. (It briefly grew
//! into a play/pause button; per-window control is a lie inside a shared
//! Chrome process — one MPRIS player owns the whole process, so buttons
//! cross-talked — and Max cut the whole idea rather than keep the confusion.)
//!
//! Polls PipeWire (`pw-dump`) for **running, unmuted** `Stream/Output/Audio`
//! nodes and reports each stream's pid ancestry. The ancestry matters because
//! the pid on a stream is rarely the pid on the window: Chrome plays audio
//! from a child process of the process owning the window. A tile shows the
//! speaker when its window pid appears anywhere in a sounding stream's chain.
//!
//! One `pw-dump` per [`POLL`], **only while the stage is up**, on this
//! dedicated thread — the mode costs nothing when it is not running.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use calloop::channel::Sender;
use tracing::{debug, warn};

/// Poll cadence while the stage is up. Sound starting or stopping shows on the
/// deck within this; it is status, not choreography, so a second is plenty.
const POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// One sounding stream: every pid in the owning process's ancestry.
pub struct Stream {
    pub ancestors: Vec<i64>,
}

/// Handle to the poller thread.
pub struct DeckAudio {
    active: Arc<AtomicBool>,
}

impl DeckAudio {
    /// Start or stop polling. Idempotent; the thread notices within a beat.
    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
    }
}

/// Spawn the poller. Results only flow while active; an empty result is sent
/// once when sound stops so stale speakers clear.
pub fn spawn(results: Sender<Vec<Stream>>) -> DeckAudio {
    let active = Arc::new(AtomicBool::new(false));
    let flag = active.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-deck-audio".into())
        .spawn(move || {
            let mut last_empty = true;
            loop {
                if !flag.load(Ordering::Relaxed) {
                    last_empty = true;
                    std::thread::sleep(POLL);
                    continue;
                }
                let streams = sample();
                // Quiet decks stay quiet: only the first empty result after
                // sound (or on activation) is worth a wakeup.
                let empty = streams.is_empty();
                if !(empty && last_empty) && results.send(streams).is_err() {
                    return; // event loop is gone
                }
                last_empty = empty;
                std::thread::sleep(POLL);
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn deck audio thread: {e}");
    }
    DeckAudio { active }
}

/// One `pw-dump` → the audibly sounding streams. A muted stream is silence and
/// does not count. Best effort: no PipeWire, no speakers.
fn sample() -> Vec<Stream> {
    let Ok(out) = std::process::Command::new("pw-dump").output() else {
        debug!("deck audio: pw-dump unavailable");
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    json.as_array()
        .into_iter()
        .flatten()
        .filter(|n| {
            n["info"]["props"]["media.class"].as_str() == Some("Stream/Output/Audio")
                && n["info"]["state"].as_str() == Some("running")
                && n["info"]["params"]["Props"][0]["mute"].as_bool() != Some(true)
        })
        .filter_map(|n| {
            let pid = n["info"]["props"]["application.process.id"].as_i64()?;
            Some(Stream {
                ancestors: ancestry(pid),
            })
        })
        .collect()
}

/// `pid` plus every parent above it, from `/proc/<pid>/stat` — bounded, and
/// robust against comm fields containing spaces or parens (parse after the
/// LAST `)`).
fn ancestry(pid: i64) -> Vec<i64> {
    let mut chain = Vec::with_capacity(6);
    let mut cur = pid;
    for _ in 0..12 {
        if cur <= 1 {
            break;
        }
        chain.push(cur);
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{cur}/stat")) else {
            break;
        };
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
            break;
        };
        // rest = " S ppid pgrp ..." — field 2 after the split.
        let Some(ppid) = rest.split_whitespace().nth(1).and_then(|p| p.parse().ok()) else {
            break;
        };
        cur = ppid;
    }
    chain
}

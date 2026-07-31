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

use std::time::Duration;

use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{AudioState, ContextState, Layer};

const POLL: Duration = Duration::from_secs(2);
/// media.class of a recording (microphone) stream node.
const MIC_CLASS: &str = "Stream/Input/Audio";

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
            loop {
                let (volume, muted) = read_sink_volume().await.unwrap_or((0, false));
                let state = AudioState {
                    is_mic_active: mic_active().await,
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
                tokio::time::sleep(POLL).await;
            }
        })
    }
}

/// Whether any microphone capture stream is currently running.
async fn mic_active() -> bool {
    let Ok(out) = Command::new("pw-dump").output().await else {
        return false;
    };
    mic_active_from_dump(&String::from_utf8_lossy(&out.stdout))
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

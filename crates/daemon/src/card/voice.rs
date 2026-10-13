//! The card's voice: VOICE NOTES (record, keep as an item, play) and
//! TALK-TO-TEXT (record, then have the words written into the input box).
//!
//! Sound goes through PipeWire's own tools — `pw-record` to a WAV under
//! the card's folder, `pw-play` back — as child processes, so nothing here
//! holds the microphone but for the moments it is recording. Talk-to-text
//! runs **whisper.cpp** (`whisper-cli`) on the recording once the talking
//! stops: on this machine, offline, nothing sent anywhere. Both are
//! best-effort: a missing tool is said in the line under the input box.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use tracing::{info, warn};

use super::model::{Kind, PICTURES_DIR};
use crate::App;

/// A recording in progress.
pub(super) struct Rec {
    child: Child,
    started: Instant,
    path: PathBuf,
    /// It is to be written out as words (talk-to-text), not kept as a note.
    talk: bool,
}

impl Rec {
    /// How long it has run, in whole seconds.
    pub(super) fn seconds(&self) -> u32 {
        self.started.elapsed().as_secs() as u32
    }

    /// Whether it is talk-to-text.
    pub(super) fn talk(&self) -> bool {
        self.talk
    }
}

/// A voice note being played.
pub(super) struct Playing {
    pub id: u64,
    child: Child,
    /// When the player began, how far into the note that was, and how
    /// fast it goes.
    started: Instant,
    from: f32,
    speed: f32,
}

impl Playing {
    /// How many seconds INTO THE NOTE it is now.
    pub(super) fn at(&self) -> f32 {
        self.from + self.started.elapsed().as_secs_f32() * self.speed
    }
}

/// What comes back from the thread that finishes a recording.
enum Done {
    /// A voice note: its file, how many seconds.
    Note(PathBuf, u32),
    /// The words that were said (empty: nothing was made out).
    Words(String),
    Failed(&'static str),
}

/// How often the card is redrawn while it records or plays.
const TICK_MS: u64 = 200;
const PLAY_TICK_MS: u64 = 33;
/// Shorter than this is a slip of the finger, not a note.
const SHORTEST: f32 = 0.4;

pub(super) fn tool(env: &str, name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(env).map(PathBuf::from) {
        return path.exists().then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// How fast a voice note plays: the speeds its "x1" goes round (Max,
/// 2026-10-11: *"x1 | x1.5 | x0.75"*).
pub(super) const SPEEDS: [f32; 3] = [1.0, 1.5, 0.75];

/// What a speed is called on the note.
pub(super) fn speed_name(speed: usize) -> &'static str {
    ["x1", "x1.5", "x0.75"][speed % SPEEDS.len()]
}

/// A copy of the recording at `wav` that begins `from` seconds in and
/// plays `speed` times as fast: its head as it is, its sound from there on
/// (a plain PCM recording — what `pw-record` writes), made shorter or
/// longer WITHOUT its pitch changing (`stretch`). `None`: it is not one, or
/// cannot be written.
fn tail_wav(wav: &str, from: f32, speed: f32) -> Option<PathBuf> {
    let all = std::fs::read(wav).ok()?;
    if all.len() < 44 || &all[0..4] != b"RIFF" || &all[8..12] != b"WAVE" {
        return None;
    }
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(all.get(at..at + 4)?.try_into().ok()?))
    };
    let u16_at = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(all.get(at..at + 2)?.try_into().ok()?))
    };
    // Its chunks: the format (how many bytes a second), then the sound.
    let (mut at, mut rate, mut align) = (12usize, 0u32, 1u32);
    let (mut kind, mut channels, mut hz, mut bits) = (0u16, 1u16, 0u32, 0u16);
    let data = loop {
        let (name, size) = (all.get(at..at + 4)?, u32_at(at + 4)? as usize);
        if name == b"fmt " {
            kind = u16_at(at + 8)?;
            channels = u16_at(at + 10)?;
            hz = u32_at(at + 12)?;
            rate = u32_at(at + 16)?;
            align = u16_at(at + 20)? as u32;
            bits = u16_at(at + 22)?;
        }
        if name == b"data" {
            break at + 8;
        }
        at += 8 + size + (size & 1);
    };
    if rate == 0 || align == 0 {
        return None;
    }
    let skip = ((from * rate as f32) as usize / align as usize) * align as usize;
    let start = (data + skip).min(all.len());
    let mut out = all[..data].to_vec();
    let stretched;
    let sound: &[u8] = if (speed - 1.0).abs() < 0.01 {
        &all[start..]
    } else if kind == 1 && bits == 16 && channels > 0 && hz > 0 {
        let samples: Vec<i16> = all[start..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        stretched = stretch(&samples, channels as usize, hz, speed)
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<u8>>();
        &stretched
    } else {
        // (Not sound this can stretch: it is played at another rate — its
        // pitch goes with it.)
        let fmt = at_fmt(&all)?;
        let faster = |v: u32| ((v as f32 * speed) as u32).to_le_bytes();
        out[fmt + 12..fmt + 16].copy_from_slice(&faster(hz));
        out[fmt + 16..fmt + 20].copy_from_slice(&faster(rate));
        &all[start..]
    };
    // (The two sizes in its head, for what is left.)
    out[4..8].copy_from_slice(&((data - 8 + sound.len()) as u32).to_le_bytes());
    let size_at = data - 4;
    out[size_at..data].copy_from_slice(&(sound.len() as u32).to_le_bytes());
    out.extend_from_slice(sound);
    let path =
        std::env::temp_dir().join(format!("waverunner-card-from-{}.wav", std::process::id()));
    std::fs::write(&path, out).ok()?;
    Some(path)
}

/// Where a recording's `fmt ` chunk begins.
fn at_fmt(all: &[u8]) -> Option<usize> {
    let mut at = 12usize;
    loop {
        let name = all.get(at..at + 4)?;
        let size = u32::from_le_bytes(all.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        if name == b"fmt " {
            return Some(at);
        }
        at += 8 + size + (size & 1);
    }
}

/// `samples` (16-bit, `channels` of them side by side, `hz` a second)
/// `speed` times as fast, AT THE SAME PITCH: short windows of the sound
/// are laid down again closer together or further apart, each taken from
/// the spot near where it should come from that carries on best from the
/// one before (WSOLA) — a voice stays a voice, not a chipmunk.
fn stretch(samples: &[i16], channels: usize, hz: u32, speed: f32) -> Vec<i16> {
    let frames = samples.len() / channels;
    // (A window of 30 ms, laid every 15; the spot looked for within 8 ms.)
    let win = ((hz as usize * 30 / 1000) & !1).max(2);
    let hop = win / 2;
    let reach = (hz as usize * 8 / 1000).max(1);
    if frames < 2 * win {
        return samples.to_vec();
    }
    // One channel of it, thinned, to compare spots by.
    let thin = (hz as usize / 6000).max(1);
    let mono: Vec<f32> = (0..frames / thin)
        .map(|i| {
            let f = i * thin * channels;
            samples[f..f + channels]
                .iter()
                .map(|s| *s as f32)
                .sum::<f32>()
                / channels as f32
        })
        .collect();
    let like = |a: usize, b: usize| -> f32 {
        let (a, b, n) = (a / thin, b / thin, hop / thin);
        if a + n > mono.len() || b + n > mono.len() {
            return f32::MIN;
        }
        let (mut dot, mut power) = (0.0f32, 1.0f32);
        for i in 0..n {
            dot += mono[a + i] * mono[b + i];
            power += mono[b + i] * mono[b + i];
        }
        dot / power.sqrt()
    };
    let shape: Vec<f32> = (0..win)
        .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / win as f32).cos())
        .collect();
    let count = ((frames - win) as f32 / (hop as f32 * speed)) as usize + 1;
    let mut out = vec![0.0f32; (count * hop + win) * channels];
    let mut last = 0usize;
    for k in 0..count {
        let ideal = (k as f32 * hop as f32 * speed) as usize;
        let from = if k == 0 {
            0
        } else {
            // (What would have come next, had nothing been moved.)
            let next = last + hop;
            let (lo, hi) = (
                ideal.saturating_sub(reach),
                (ideal + reach).min(frames - win),
            );
            (lo..=hi.max(lo))
                .step_by(thin)
                .max_by(|a, b| like(next, *a).total_cmp(&like(next, *b)))
                .unwrap_or(ideal)
                .min(frames - win)
        };
        last = from;
        for (n, w) in shape.iter().enumerate() {
            for c in 0..channels {
                out[(k * hop + n) * channels + c] += samples[(from + n) * channels + c] as f32 * w;
            }
        }
    }
    out.into_iter()
        .map(|v| v.clamp(i16::MIN as f32, i16::MAX as f32) as i16)
        .collect()
}

/// The speech engine and its model, when both are there:
/// `WAVERUNNER_WHISPER` / `whisper-cli` on the PATH / the data folder's
/// `whisper/engine/bin/whisper-cli`, and `WAVERUNNER_WHISPER_MODEL` / the
/// first `ggml-*.bin` under the data folder's `whisper/`.
pub(super) fn engine() -> Option<(PathBuf, PathBuf)> {
    // (…or the one linked beside the model: `whisper/engine` → its package.)
    let beside = crate::persist::data_path("whisper").join("engine/bin/whisper-cli");
    let cli =
        tool("WAVERUNNER_WHISPER", "whisper-cli").or_else(|| beside.is_file().then_some(beside))?;
    let model = match std::env::var_os("WAVERUNNER_WHISPER_MODEL").map(PathBuf::from) {
        Some(model) => model.exists().then_some(model)?,
        None => {
            let mut models: Vec<PathBuf> = std::fs::read_dir(crate::persist::data_path("whisper"))
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("ggml-") && n.ends_with(".bin"))
                })
                .collect();
            models.sort();
            models.into_iter().next()?
        }
    };
    Some((cli, model))
}

/// Start recording the microphone to a new file under the card's folder
/// (`talk`: to be written out, which needs the speech engine) — or why not.
pub(super) fn recorder(talk: bool) -> Result<(Child, PathBuf), &'static str> {
    if talk && engine().is_none() {
        return Err("Talk-to-text needs the speech engine (whisper), which is not installed");
    }
    if mic_muted() {
        return Err("The microphone is muted. Unmute it and press again");
    }
    let recorder = tool("WAVERUNNER_PW_RECORD", "pw-record").ok_or("No recorder found (pw-record)")?;
    let dir = crate::persist::data_path(PICTURES_DIR);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("card: no folder for a recording ({e})");
        return Err("The recorder did not start");
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("voice-{stamp}.wav"));
    // (16 kHz mono is what speech needs, and what the engine reads.)
    let mut command = Command::new(recorder);
    command
        .args(["--rate", "16000", "--channels", "1", "--format", "s16"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The recorder ENDS WITH THE DOCK. Left to itself it went on recording
    // the microphone, for ever, into a file nobody would read, when the dock
    // was killed or crashed while a key was held.
    // SAFETY: `prctl` is async-signal-safe and touches nothing of ours.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            // (The signal as a full word: `prctl` takes its arguments as
            // longs, and an int left the upper half to chance — the kernel
            // then refused it, silently. Measured: the recorder lived on.)
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGINT as libc::c_ulong);
            Ok(())
        });
    }
    command
        .spawn()
        .map(|child| (child, path))
        .map_err(|e| {
            warn!("card: the recorder did not start ({e})");
            "The recorder did not start"
        })
}

/// Whether the microphone the system records from is muted (`wpctl`; not
/// known = not muted: the recording itself is checked afterwards).
fn mic_muted() -> bool {
    let Some(wpctl) = tool("WAVERUNNER_WPCTL", "wpctl") else {
        return false;
    };
    Command::new(wpctl)
        .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("MUTED"))
}

/// A recording whose loudest sample is under this is silence.
pub(super) const QUIET: i32 = 120;

/// What a recording holds, before anything is made of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Heard {
    /// Something to keep, or to write out.
    Sound,
    /// Nothing: silence, or (for talk) an even hiss with no voice in it.
    Nothing,
    /// The microphone is turned up so far that it gives NOISE — its hiss
    /// fills the recording, or clips. The level of that hiss (the quiet
    /// tenth of the recording, RMS).
    TooLoud(f32),
}

/// How loud the quiet tenth of a recording is when the microphone itself is
/// the noise, and the share of clipped samples that says the same.
const HISS: f32 = 1500.0;
const CLIPPED: f32 = 0.05;
/// …and how even that hiss is: its loud tenth under this many times its
/// quiet one.
const EVEN: f32 = 2.0;
/// A voice stands out of the room it is in: the loud tenth of talk is this
/// many times its quiet tenth, and at least this loud.
const VOICE_OVER: f32 = 3.0;
const VOICE_LEAST: f32 = 150.0;
/// Where a microphone that was too loud is turned down to: the level of its
/// hiss afterwards.
const HISS_AFTER: f32 = 200.0;
/// Above this volume a microphone's own amplifier is in play (on the Acer
/// the boost comes in past 0.40).
const MIC_BOOSTED: f32 = 0.45;

/// Listen to the 16-bit recording at `wav` before it is used (`talk`: it is
/// to be written out, so it must have a voice in it).
///
/// Measured on the laptops, 2026-10-13. An Acer E5-573 at its microphone's
/// default level (100 %: +30 dB capture and +30 dB boost) records hiss at
/// full scale, a fifth of it clipped — and the speech engine, given hiss,
/// WRITES SOMETHING ("Bu konjun ne lan bu sorun?", "¡Y no se lo entesan
/// bien!"), which was pasted into the window. At 22 % the same microphone
/// hears a sentence perfectly. A quiet room on a MacBook Air is 2–200 of
/// RMS, flat; a sentence is 300–19000 at its loud tenth over 2–150 at its
/// quiet one.
pub(super) fn heard(wav: &PathBuf, talk: bool) -> Heard {
    let Ok(bytes) = std::fs::read(wav) else {
        return Heard::Nothing;
    };
    // (Past the WAV's head: 44 bytes as the recorder writes it.)
    let samples: Vec<i16> = bytes
        .get(44..)
        .unwrap_or(&[])
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes([s[0], s[1]]))
        .collect();
    // (30 ms at the recorder's 16 kHz.)
    const FRAME: usize = 480;
    let mut levels: Vec<f32> = samples
        .chunks_exact(FRAME)
        .map(|f| (f.iter().map(|&x| f32::from(x) * f32::from(x)).sum::<f32>() / FRAME as f32).sqrt())
        .collect();
    if levels.len() < 5 {
        return Heard::Nothing;
    }
    levels.sort_by(f32::total_cmp);
    let (quiet, loud) = (levels[levels.len() / 10], levels[levels.len() * 9 / 10]);
    let clipped = samples.iter().filter(|&&x| x.unsigned_abs() >= 32700).count() as f32 / samples.len() as f32;
    // (An amplifier's hiss is EVEN — its loud tenth 1.3 times its quiet one
    // on the Acer; music, a crowd, a voice close by rise and fall. Only talk
    // is judged: a voice note of a loud place is the owner's to keep.)
    if talk && loud < quiet * EVEN && (quiet > HISS || (clipped > CLIPPED && quiet > HISS / 2.0)) {
        return Heard::TooLoud(quiet);
    }
    let peak = samples.iter().map(|&x| i32::from(x).abs()).max().unwrap_or(0);
    if peak < QUIET || (talk && (loud < VOICE_LEAST || loud < quiet * VOICE_OVER)) {
        return Heard::Nothing;
    }
    Heard::Sound
}

/// The microphone gave noise at `hiss` (see [`Heard::TooLoud`]): turn it
/// down to where its hiss is small — its volume is cubic, so the level goes
/// with the cube of it. What to say of it.
pub(super) fn mic_turn_down(hiss: f32) -> &'static str {
    let turned = (|| {
        let wpctl = tool("WAVERUNNER_WPCTL", "wpctl")?;
        let out = Command::new(&wpctl)
            .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        // (A decimal comma, where the owner's numbers are written with one.)
        let now: f32 = String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1)?.replace(',', ".").parse().ok()?;
        // (Only a microphone that is turned UP can be its own noise: below
        // this its boost is off, and a recording this loud is the ROOM —
        // music, a crowd. That is not ours to turn down, again and again.)
        if now <= MIC_BOOSTED {
            return None;
        }
        let to = mic_level_for(now, hiss);
        if to >= now {
            return None;
        }
        info!("card: the microphone hisses at {hiss:.0} — turned down from {now:.2} to {to:.2}");
        Command::new(&wpctl)
            .args(["set-volume", "@DEFAULT_AUDIO_SOURCE@", &format!("{to:.2}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
    })();
    if turned.is_some() {
        "The microphone was too loud: turned down. Say it again"
    } else {
        "Too much noise to make anything out"
    }
}

/// The volume a microphone at `now` whose hiss is `hiss` is turned down to.
fn mic_level_for(now: f32, hiss: f32) -> f32 {
    // (A recording that clips hides how loud it really is: it is at least
    // this, so the step down is at least this big — and never a small one.)
    (now * (HISS_AFTER / hiss.max(HISS)).cbrt()).clamp(0.15, (now * 0.8).max(0.15))
}

/// Write out what is said in the recording at `wav`.
pub(super) fn transcribe(
    cli: &PathBuf,
    model: &PathBuf,
    wav: &PathBuf,
    // Where its process id is left while it runs, for whoever may stop it.
    running: Option<&std::sync::atomic::AtomicI32>,
) -> Result<String, &'static str> {
    let mut command = Command::new(cli);
    command
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-l", "auto", "-nt", "-np"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // (It ends with the dock, as the recorder does.)
    // SAFETY: `prctl` is async-signal-safe and touches nothing of ours.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong);
            Ok(())
        });
    }
    let child = command.spawn().map_err(|_| "The speech engine did not start")?;
    if let Some(running) = running {
        running.store(child.id() as i32, std::sync::atomic::Ordering::Relaxed);
    }
    let out = child.wait_with_output();
    if let Some(running) = running {
        running.store(0, std::sync::atomic::Ordering::Relaxed);
    }
    let out = out.map_err(|_| "The speech engine did not start")?;
    if !out.status.success() {
        return Err("The speech engine could not read that");
    }
    Ok(words(&String::from_utf8_lossy(&out.stdout)))
}

/// The engine's output as one run of words: its lines joined, and the
/// marks it writes for silence and noise ("[BLANK_AUDIO]", "(music)") left
/// out.
pub(super) fn words(out: &str) -> String {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !(l.starts_with('[') && l.ends_with(']')))
        .filter(|l| !(l.starts_with('(') && l.ends_with(')')))
        .collect::<Vec<_>>()
        .join(" ")
}

impl App {
    /// The record button (a voice note) or the talk button (`talk`): start,
    /// or — pressed again — stop and keep what was said.
    pub(super) fn card_voice_button(&mut self, talk: bool) {
        match self.card.rec.as_ref().map(|r| r.talk) {
            Some(running) if running == talk => self.card_record_stop(true),
            // (The other one was running: that one is dropped for this.)
            Some(_) => {
                self.card_record_stop(false);
                self.card_record_start(talk);
            }
            None => self.card_record_start(talk),
        }
    }

    pub(super) fn card_record_start(&mut self, talk: bool) {
        match recorder(talk) {
            Ok((child, path)) => {
                info!(
                    "card: recording {}",
                    if talk { "to write it" } else { "a voice note" }
                );
                self.card.say = None;
                self.card.rec = Some(Rec {
                    child,
                    started: Instant::now(),
                    path,
                    talk,
                });
                self.card_voice_tick();
            }
            Err(why) => self.card.say = Some(why),
        }
        self.request_card_draw();
    }

    /// Stop recording. Kept: a voice note lands in the memory; talk is
    /// written into the input box. Not kept: the recording is thrown away.
    pub(super) fn card_record_stop(&mut self, keep: bool) {
        let Some(mut rec) = self.card.rec.take() else {
            return;
        };
        let secs = rec.started.elapsed().as_secs_f32();
        // (Interrupted, not killed: the recorder then closes the file as a
        // sound file is closed, its length written in its head.)
        // SAFETY: a signal to a child of ours that is still to be waited for.
        unsafe {
            libc::kill(rec.child.id() as libc::pid_t, libc::SIGINT);
        }
        let keep = keep && secs >= SHORTEST;
        let talk = rec.talk;
        let engine = if talk && keep { engine() } else { None };
        if talk && keep {
            self.card.writing = true;
        }
        let (tx, rx) = calloop::channel::channel::<Done>();
        std::thread::spawn(move || {
            let _ = rec.child.wait();
            // (Nothing but silence in it: said, and not kept — a muted or
            // dead microphone records exactly that.)
            if keep {
                let why = match heard(&rec.path, talk) {
                    Heard::Sound => None,
                    Heard::Nothing => Some("Nothing was heard. Is the microphone muted or off?"),
                    Heard::TooLoud(hiss) => Some(mic_turn_down(hiss)),
                };
                if let Some(why) = why {
                    let _ = std::fs::remove_file(&rec.path);
                    let _ = tx.send(Done::Failed(why));
                    return;
                }
            }
            let done = match (keep, talk, engine) {
                (false, _, _) => {
                    let _ = std::fs::remove_file(&rec.path);
                    return;
                }
                (true, false, _) => Done::Note(rec.path, secs.round().max(1.0) as u32),
                (true, true, Some((cli, model))) => {
                    let said = transcribe(&cli, &model, &rec.path, None);
                    let _ = std::fs::remove_file(&rec.path);
                    match said {
                        Ok(text) => Done::Words(text),
                        Err(why) => Done::Failed(why),
                    }
                }
                (true, true, None) => {
                    let _ = std::fs::remove_file(&rec.path);
                    Done::Failed("The speech engine is not installed")
                }
            };
            let _ = tx.send(done);
        });
        let waiting = self
            .loop_handle
            .insert_source(rx, |event, _, app: &mut App| {
                let calloop::channel::Event::Msg(done) = event else {
                    return;
                };
                app.card.writing = false;
                match done {
                    Done::Note(path, secs) => {
                        let body = format!("Voice note · {}:{:02}", secs / 60, secs % 60);
                        app.card_push(
                            Kind::Voice,
                            body,
                            Some(path.to_string_lossy().into_owned()),
                            true,
                            secs as f32,
                            None,
                        );
                        // (Recorded with Space held: it asks for its title
                        // at once — Max, 2026-10-10.)
                        if std::mem::take(&mut app.card.name_next) {
                            if let Some(id) = app.card.items.last().map(|it| it.id) {
                                app.card_edit(id);
                            }
                        }
                    }
                    Done::Words(text) if text.is_empty() => {
                        app.card.say = Some("Nothing was made out")
                    }
                    // (On voice-to-text's page what was said goes into
                    // the WINDOW, and stays on the page as its last thing.)
                    Done::Words(text) if app.card.page == super::Page::Talk => app.card_said(&text),
                    Done::Words(text) => {
                        app.card_say_in(&text);
                    }
                    Done::Failed(why) => app.card.say = Some(why),
                }
                app.sync_card_input();
                app.request_card_draw();
            });
        if waiting.is_err() {
            warn!("card: cannot wait for the recording");
            self.card.writing = false;
        }
        self.request_card_draw();
    }

    /// A click on a voice note's line (`along`: how far along it, 0..1):
    /// PLAYING, it plays from there; not playing, it is paused there, and
    /// its button plays from that spot (Max, 2026-10-11).
    pub(super) fn card_play_from(&mut self, id: u64, along: f32) {
        let secs = self.card.item(id).map_or(0.0, |it| it.aspect.max(1.0));
        let at = (along.clamp(0.0, 0.98) * secs).max(0.0);
        if self.card.playing.as_ref().is_none_or(|p| p.id != id) {
            self.card.paused = Some((id, at));
            self.request_card_draw();
            return;
        }
        if let Some(mut p) = self.card.playing.take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
        // (Should the player not start, it is paused there.)
        self.card.paused = Some((id, at));
        self.card_play_at(id, at);
        if self.card.playing.is_some() {
            self.card.paused = None;
        }
        self.request_card_draw();
    }

    /// A voice note's "x1": the next speed (they go round — `SPEEDS`), for
    /// every note. One that is playing goes on from where it is, that fast.
    pub(super) fn card_speed(&mut self) {
        self.card.voice_speed = (self.card.voice_speed + 1) % SPEEDS.len();
        if let Some(mut p) = self.card.playing.take() {
            let (id, at) = (p.id, p.at());
            let _ = p.child.kill();
            let _ = p.child.wait();
            self.card.paused = Some((id, at));
            self.card_play_at(id, at);
            if self.card.playing.is_some() {
                self.card.paused = None;
            }
        }
        self.request_card_draw();
    }

    /// A voice note's button: it plays; playing, it PAUSES (Max,
    /// 2026-10-11: play/pause, not play/stop) and the next click goes on
    /// from there. (The player cannot be held: it is let go, and started
    /// again from where it was — `card_play_at`.)
    pub(super) fn card_play(&mut self, id: u64) {
        let was = self.card.playing.take().map(|mut p| {
            let _ = p.child.kill();
            let _ = p.child.wait();
            (p.id, p.at())
        });
        let paused = self.card.paused.take();
        // (Drawn only once it is known what it is now: drawn in between —
        // neither playing nor paused — its line was at the start for a
        // frame, a blink.)
        if let Some((_, at)) = was.filter(|w| w.0 == id) {
            self.card.paused = Some((id, at));
            self.request_card_draw();
            return;
        }
        let from = paused.filter(|p| p.0 == id).map_or(0.0, |p| p.1);
        // (Should the player not start, it stays paused where it was.)
        self.card.paused = paused.filter(|p| p.0 == id);
        self.card_play_at(id, from);
        if self.card.playing.is_some() {
            self.card.paused = None;
        }
        self.request_card_draw();
    }

    /// Play voice note `id` from `from` seconds in. (The player has no way
    /// to start in the middle: it is given a copy of the recording that
    /// begins there — `tail_wav`.)
    fn card_play_at(&mut self, id: u64, from: f32) {
        let Some(path) = self.card.item(id).and_then(|it| it.path.clone()) else {
            return;
        };
        let Some(player) = tool("WAVERUNNER_PW_PLAY", "pw-play") else {
            self.card.say = Some("No player found (pw-play)");
            return;
        };
        let speed = SPEEDS[self.card.voice_speed % SPEEDS.len()];
        let whole = from <= 0.05 && (speed - 1.0).abs() < 0.01;
        let (path, from, speed) = match (!whole).then(|| tail_wav(&path, from, speed)).flatten() {
            Some(tail) => (tail.to_string_lossy().into_owned(), from, speed),
            None => (path, 0.0, 1.0),
        };
        match Command::new(player)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.card.playing = Some(Playing {
                    id,
                    child,
                    started: Instant::now(),
                    from,
                    speed,
                });
                self.card_voice_tick();
            }
            Err(e) => warn!("card: the player did not start ({e})"),
        }
    }

    /// While something records or plays, the card is redrawn a few times a
    /// second (the seconds count, the played part of the wave), and a note
    /// that has played to its end is let go.
    fn card_voice_tick(&mut self) {
        // (ONE of these runs at a time. Each start of the player asked for
        // another — a note sought or sped up while it played had two, then
        // three… each redrawing the card thirty times a second.)
        if std::mem::replace(&mut self.card.voice_ticking, true) {
            return;
        }
        // (A note that PLAYS is redrawn every frame or so: its line glides,
        // and where it is paused is where it was seen — at a few times a
        // second it went in steps, and jumped ahead when it was paused.)
        let every = if self.card.playing.is_some() {
            PLAY_TICK_MS
        } else {
            TICK_MS
        };
        self.after_ms(every, |app| {
            app.card.voice_ticking = false;
            if let Some(playing) = app.card.playing.as_mut() {
                if !matches!(playing.child.try_wait(), Ok(None)) {
                    app.card.playing = None;
                }
            }
            app.request_card_draw();
            if app.card.rec.is_some() || app.card.playing.is_some() {
                app.card_voice_tick();
            }
        });
    }

    /// The card is going away: nothing goes on recording or playing.
    pub(super) fn card_voice_quiet(&mut self) {
        self.card_record_stop(false);
        self.card.talk_key = None;
        self.card.paused = None;
        if let Some(mut playing) = self.card.playing.take() {
            let _ = playing.child.kill();
            let _ = playing.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note played faster or slower is that much shorter or longer, and
    /// its PITCH is what it was (a tone of 200 Hz crosses zero as often a
    /// second as before).
    /// The copy the player is given: from a second in, half as fast again
    /// — a recording as `pw-record` writes one (16 kHz, one channel).
    #[test]
    fn the_copy_played_begins_there_and_is_that_fast() {
        let hz = 16000u32;
        let sound: Vec<u8> = (0..hz * 4)
            .map(|n| ((n as f32 * 0.07).sin() * 8000.0) as i16)
            .flat_map(i16::to_le_bytes)
            .collect();
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + sound.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(hz.to_le_bytes());
        wav.extend((hz * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((sound.len() as u32).to_le_bytes());
        wav.extend(&sound);
        let path = std::env::temp_dir().join(format!("card-speed-test-{}.wav", std::process::id()));
        std::fs::write(&path, &wav).unwrap();
        let copy = tail_wav(path.to_str().unwrap(), 1.0, 1.5).unwrap();
        let out = std::fs::read(&copy).unwrap();
        let _ = (std::fs::remove_file(&path), std::fs::remove_file(&copy));
        // (Three seconds are left, played in two.)
        let secs = (out.len() - 44) as f32 / (hz * 2) as f32;
        assert!((secs - 2.0).abs() < 0.1, "{secs}");
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(out[40..44].try_into().unwrap()) as usize,
            out.len() - 44
        );
    }

    #[test]
    fn a_stretched_note_keeps_its_pitch() {
        let hz = 16000u32;
        let tone: Vec<i16> = (0..hz * 2)
            .map(|n| ((n as f32 * 200.0 * std::f32::consts::TAU / hz as f32).sin() * 9000.0) as i16)
            .collect();
        let crossings = |s: &[i16]| {
            s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f32 / s.len() as f32
        };
        for speed in [1.5f32, 0.75] {
            let out = stretch(&tone, 1, hz, speed);
            let long = out.len() as f32 / tone.len() as f32;
            assert!((long - 1.0 / speed).abs() < 0.05, "{speed}: {long}");
            // (Its middle: the ends fade in and out.)
            let mid = &out[out.len() / 4..out.len() * 3 / 4];
            let was = crossings(&tone);
            assert!(
                (crossings(mid) - was).abs() / was < 0.04,
                "{speed}: {}",
                crossings(mid) / was
            );
        }
    }

    #[test]
    fn a_recording_can_be_cut_to_begin_later() {
        // One second of 16 kHz mono 16-bit sound: 32000 bytes, each sample its own number.
        let sound: Vec<u8> = (0..16000u32)
            .flat_map(|n| (n as u16).to_le_bytes())
            .collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + sound.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(sound.len() as u32).to_le_bytes());
        wav.extend_from_slice(&sound);
        let path = std::env::temp_dir().join(format!("card-cut-test-{}.wav", std::process::id()));
        std::fs::write(&path, &wav).unwrap();
        let cut = tail_wav(path.to_str().unwrap(), 0.5, 1.0).unwrap();
        let back = std::fs::read(&cut).unwrap();
        // Half of it is left, beginning at the sample half a second in.
        assert_eq!(back.len(), 44 + 16000);
        assert_eq!(u32::from_le_bytes(back[40..44].try_into().unwrap()), 16000);
        assert_eq!(u16::from_le_bytes(back[44..46].try_into().unwrap()), 8000);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(cut);
        assert!(tail_wav("/nonexistent.wav", 1.0, 1.0).is_none());
    }

    #[test]
    fn the_engines_output_is_one_run_of_words() {
        let out = "\n Hello, this is a test.\n And a second line.\n[BLANK_AUDIO]\n (music)\n\n";
        assert_eq!(words(out), "Hello, this is a test. And a second line.");
        assert_eq!(words(" [BLANK_AUDIO] \n"), "");
    }

    #[test]
    fn a_recording_of_nothing_is_known_for_silence() {
        let dir = std::env::temp_dir().join(format!("card-voice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = |name: &str, samples: &[i16]| {
            let path = dir.join(name);
            let mut bytes = vec![0u8; 44];
            bytes.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
            std::fs::write(&path, bytes).unwrap();
            path
        };
        assert_eq!(heard(&dir.join("none.wav"), false), Heard::Nothing);
        // What the laptops' microphones gave (2026-10-13), in kind: a
        // sentence in a quiet room is heard; the room alone is not; an even
        // hiss is not TALK (the engine writes nonsense from it) though it
        // can be kept as a note; a microphone that fills the recording with
        // its own noise is told apart, with how loud that noise is.
        let tone = |amp: f32, n: usize| -> Vec<i16> { (0..n).map(|i| (amp * (i as f32 * 0.3).sin()) as i16).collect() };
        let hiss = |amp: i32, n: usize| -> Vec<i16> {
            let mut x = 12345u32;
            (0..n)
                .map(|_| {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    (((x >> 16) as i32 % (2 * amp + 1)) - amp) as i16
                })
                .collect()
        };
        let mut sentence = hiss(30, 8000);
        sentence.extend(tone(6000.0, 24000));
        sentence.extend(hiss(30, 8000));
        assert_eq!(heard(&wav("sentence.wav", &sentence), true), Heard::Sound);
        assert_eq!(heard(&wav("room.wav", &hiss(40, 40000)), true), Heard::Nothing);
        assert_eq!(heard(&wav("even.wav", &hiss(900, 40000)), true), Heard::Nothing);
        assert_eq!(heard(&wav("even-note.wav", &hiss(900, 40000)), false), Heard::Sound);
        assert!(matches!(heard(&wav("noise.wav", &hiss(32767, 40000)), true), Heard::TooLoud(h) if h > 10000.0));
        assert_eq!(heard(&wav("noise-note.wav", &hiss(32767, 40000)), false), Heard::Sound);
        // A LOUD ROOM is not a microphone to turn down: it rises and falls.
        let mut room = hiss(4000, 12000);
        room.extend(tone(20000.0, 16000));
        room.extend(hiss(4000, 12000));
        assert_eq!(heard(&wav("loud-room.wav", &room), true), Heard::Sound);
        assert_eq!(heard(&wav("short.wav", &[0, 900, -4000, 12]), true), Heard::Nothing);
        // The Acer's microphone at 100 % is brought to about a fifth, where
        // it heard the sentence; one a little hissy is taken down a step.
        assert!((mic_level_for(1.0, 23000.0) - 0.205).abs() < 0.01);
        assert!((mic_level_for(0.5, 1600.0) - 0.25).abs() < 0.01);
        assert_eq!(mic_level_for(0.16, 9000.0), 0.15);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

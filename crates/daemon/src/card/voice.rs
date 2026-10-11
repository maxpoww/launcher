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
use std::time::{Duration, Instant};

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
    pub started: Instant,
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

/// A copy of the recording at `wav` that begins `from` seconds in: its
/// head as it is, its sound from there on (a plain PCM recording — what
/// `pw-record` writes). `None`: it is not one, or cannot be written.
fn tail_wav(wav: &str, from: f32) -> Option<PathBuf> {
    let all = std::fs::read(wav).ok()?;
    if all.len() < 44 || &all[0..4] != b"RIFF" || &all[8..12] != b"WAVE" {
        return None;
    }
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(all.get(at..at + 4)?.try_into().ok()?))
    };
    // Its chunks: the format (how many bytes a second), then the sound.
    let (mut at, mut rate, mut align) = (12usize, 0u32, 1u32);
    let data = loop {
        let (name, size) = (all.get(at..at + 4)?, u32_at(at + 4)? as usize);
        if name == b"fmt " {
            rate = u32_at(at + 16)?;
            align = u16::from_le_bytes(all.get(at + 20..at + 22)?.try_into().ok()?) as u32;
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
    let sound = &all[start..];
    let mut out = all[..data].to_vec();
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
const QUIET: i32 = 120;

/// The loudest sample in the 16-bit recording at `wav` (0: none, or
/// unreadable).
pub(super) fn loudest(wav: &PathBuf) -> i32 {
    let Ok(bytes) = std::fs::read(wav) else {
        return 0;
    };
    // (Past the WAV's head: 44 bytes as the recorder writes it.)
    bytes
        .get(44..)
        .unwrap_or(&[])
        .chunks_exact(2)
        .map(|s| i32::from(i16::from_le_bytes([s[0], s[1]])).abs())
        .max()
        .unwrap_or(0)
}

/// Write out what is said in the recording at `wav`.
pub(super) fn transcribe(
    cli: &PathBuf,
    model: &PathBuf,
    wav: &PathBuf,
) -> Result<String, &'static str> {
    let out = Command::new(cli)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-l", "auto", "-nt", "-np"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "The speech engine did not start")?;
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
        if talk && engine().is_none() {
            self.card.say =
                Some("Talk-to-text needs the speech engine (whisper), which is not installed");
            self.request_card_draw();
            return;
        }
        if mic_muted() {
            self.card.say = Some("The microphone is muted. Unmute it and press again");
            self.request_card_draw();
            return;
        }
        let Some(recorder) = tool("WAVERUNNER_PW_RECORD", "pw-record") else {
            self.card.say = Some("No recorder found (pw-record)");
            self.request_card_draw();
            return;
        };
        let dir = crate::persist::data_path(PICTURES_DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("card: no folder for a recording ({e})");
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = dir.join(format!("voice-{stamp}.wav"));
        // (16 kHz mono is what speech needs, and what the engine reads.)
        let child = Command::new(recorder)
            .args(["--rate", "16000", "--channels", "1", "--format", "s16"])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => {
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
            Err(e) => {
                warn!("card: the recorder did not start ({e})");
                self.card.say = Some("The recorder did not start");
            }
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
            if keep && loudest(&rec.path) < QUIET {
                let _ = std::fs::remove_file(&rec.path);
                let _ = tx.send(Done::Failed(
                    "Nothing was heard. Is the microphone muted or off?",
                ));
                return;
            }
            let done = match (keep, talk, engine) {
                (false, _, _) => {
                    let _ = std::fs::remove_file(&rec.path);
                    return;
                }
                (true, false, _) => Done::Note(rec.path, secs.round().max(1.0) as u32),
                (true, true, Some((cli, model))) => {
                    let said = transcribe(&cli, &model, &rec.path);
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

    /// A voice note's button: it plays; playing, it PAUSES (Max,
    /// 2026-10-11: play/pause, not play/stop) and the next click goes on
    /// from there. (The player cannot be held: it is let go, and started
    /// again from where it was — `card_play_at`.)
    pub(super) fn card_play(&mut self, id: u64) {
        let was = self.card.playing.take().map(|mut p| {
            let _ = p.child.kill();
            let _ = p.child.wait();
            (p.id, p.started.elapsed().as_secs_f32())
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
        let (path, from) = match (from > 0.05).then(|| tail_wav(&path, from)).flatten() {
            Some(tail) => (tail.to_string_lossy().into_owned(), from),
            None => (path, 0.0),
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
                    // (As if it had begun that long ago: its line and its
                    // clock go on from there.)
                    started: Instant::now() - Duration::from_secs_f32(from),
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
        // (A note that PLAYS is redrawn every frame or so: its line glides,
        // and where it is paused is where it was seen — at a few times a
        // second it went in steps, and jumped ahead when it was paused.)
        let every = if self.card.playing.is_some() {
            PLAY_TICK_MS
        } else {
            TICK_MS
        };
        self.after_ms(every, |app| {
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
        let cut = tail_wav(path.to_str().unwrap(), 0.5).unwrap();
        let back = std::fs::read(&cut).unwrap();
        // Half of it is left, beginning at the sample half a second in.
        assert_eq!(back.len(), 44 + 16000);
        assert_eq!(u32::from_le_bytes(back[40..44].try_into().unwrap()), 16000);
        assert_eq!(u16::from_le_bytes(back[44..46].try_into().unwrap()), 8000);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(cut);
        assert!(tail_wav("/nonexistent.wav", 1.0).is_none());
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
        assert!(loudest(&wav("quiet.wav", &[0, 3, -2, 0])) < QUIET);
        assert!(loudest(&wav("said.wav", &[0, 900, -4000, 12])) >= QUIET);
        assert_eq!(loudest(&dir.join("none.wav")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

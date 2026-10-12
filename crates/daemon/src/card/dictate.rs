//! DICTATION WITHOUT THE CARD: a key held, talk, let go — and what was said
//! is written into the window (Max, 2026-10-12: *"press Super + hold Space
//! to just do voice to text but without the card. I want to see the duration
//! on the pill, 1 2 3 4s, and when I leave it, a loading animation on the
//! pill, and then paste the text"*).
//!
//! The key is ONE key with two uses, told apart here and not by the
//! compositor (which only says "down" and "up"): let go before `HOLD_MS` it
//! was a TAP, and the card comes up at voice-to-text's page; held past it,
//! it is dictation. The recorder starts at "down" either way, so the first
//! word of something dictated is not lost to the wait that tells the two
//! apart; a tap's third of a second of sound is thrown away.
//!
//! What is seen is the TASK pill (`tasks.rs`): "Listening" and the seconds
//! while the key is down, "Writing" with its fill going to and fro while the
//! speech engine works, then nothing — the words are in the window. Its
//! Cancel ends either. What goes wrong is said on the same pill, for a
//! moment. Sound and engine are the card's own (`voice.rs`): recorded and
//! written out on this machine.

use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tracing::{info, warn};

use super::{voice, HOLD_MS};
use crate::tasks::{Counts, Report};
use crate::App;

/// The key is down.
pub(super) struct Dictation {
    /// The recorder and what it writes — or why nothing is recorded.
    rec: Result<(Child, PathBuf), &'static str>,
    /// Since when: also WHICH press this is, for the timers that outlive it.
    since: Instant,
    /// Its task on the bar, once the key has been down long enough to be
    /// dictation: the task's id, and the flag its Cancel raises.
    task: Option<(u64, Arc<AtomicBool>)>,
}

/// How often the seconds are looked at.
const TICK_MS: u64 = 200;
/// A recording ends by itself after this long: a key whose "up" never came
/// (the session locked meanwhile) does not record for ever.
const LONGEST_S: u64 = 180;
/// How long something that went wrong stays said on the bar.
const SAID_MS: u64 = 2600;
/// From the words being offered to the paste key: the selection is the
/// dock's before the window asks for it.
const PASTE_AFTER_MS: u64 = 80;

impl App {
    /// The key went down.
    pub(super) fn dictate_down(&mut self) -> &'static str {
        // (Down already — a key that repeats — or the card is recording.)
        if self.card.dictation.is_some() || self.card.rec.is_some() {
            return "busy";
        }
        let since = Instant::now();
        self.card.dictation = Some(Dictation { rec: voice::recorder(true), since, task: None });
        self.after_ms(HOLD_MS, move |app| app.dictate_held(since));
        "down"
    }

    /// Still down after `HOLD_MS`: it is dictation, and the bar says so.
    fn dictate_held(&mut self, since: Instant) {
        let Some(d) = self.card.dictation.as_mut().filter(|d| d.since == since) else {
            return;
        };
        if let Err(why) = d.rec {
            self.card.dictation = None;
            self.dictate_say(why);
            return;
        }
        info!("dictation: listening");
        let (id, stop) = self.tasks.begin("Listening");
        self.tasks.report(Report::Counts { id, counts: Counts::Seconds }, Instant::now());
        if let Some(d) = self.card.dictation.as_mut() {
            d.task = Some((id, stop));
        }
        self.tasks_changed();
        self.dictate_tick(since);
    }

    /// While it listens: the seconds on the pill, its Cancel, its limit.
    fn dictate_tick(&mut self, since: Instant) {
        self.after_ms(TICK_MS, move |app| {
            let Some(d) = app.card.dictation.as_ref().filter(|d| d.since == since) else {
                return;
            };
            let Some((id, stop)) = d.task.clone() else {
                return;
            };
            let secs = since.elapsed().as_secs();
            if stop.load(Ordering::Relaxed) {
                info!("dictation: cancelled from the bar");
                app.dictate_drop();
            } else if secs >= LONGEST_S {
                app.dictate_up();
            } else {
                if app.tasks.report(Report::Progress { id, done: secs, total: 0 }, Instant::now()) {
                    app.tasks_changed();
                }
                app.dictate_tick(since);
            }
        });
    }

    /// Throw away what is being recorded.
    fn dictate_drop(&mut self) {
        let Some(d) = self.card.dictation.take() else {
            return;
        };
        if let Ok((mut child, path)) = d.rec {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
                let _ = std::fs::remove_file(path);
            });
        }
        if let Some((id, _)) = d.task {
            if self.tasks.cancel(id) {
                self.tasks_changed();
            }
        }
    }

    /// The key came up. A tap: the card, at voice-to-text. Held: what was
    /// said is written out, then pasted into the window.
    pub(super) fn dictate_up(&mut self) -> String {
        let Some(task) = self.card.dictation.as_ref().map(|d| d.task.clone()) else {
            return "not down".to_owned();
        };
        let Some((id, stop)) = task else {
            self.dictate_drop();
            return self.card_command("at voice");
        };
        let Some(Dictation { rec: Ok((mut child, path)), .. }) = self.card.dictation.take() else {
            return "nothing recorded".to_owned();
        };
        // (Interrupted, not killed: the recorder closes its file whole.)
        // SAFETY: a signal to a child of ours that is still to be waited for.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGINT);
        }
        let now = Instant::now();
        self.tasks.report(Report::Label { id, label: "Writing".to_owned() }, now);
        self.tasks.report(Report::Counts { id, counts: Counts::Busy }, now);
        self.tasks_changed();
        let engine = voice::engine();
        let (tx, rx) = calloop::channel::channel::<Result<String, &'static str>>();
        std::thread::spawn(move || {
            let _ = child.wait();
            let said = if voice::loudest(&path) < voice::QUIET {
                Err("Nothing was heard. Is the microphone muted or off?")
            } else if let Some((cli, model)) = engine {
                voice::transcribe(&cli, &model, &path)
            } else {
                Err("The speech engine is not installed")
            };
            let _ = std::fs::remove_file(&path);
            let _ = tx.send(said);
        });
        let waiting = self.loop_handle.insert_source(rx, move |event, _, app: &mut App| {
            let calloop::channel::Event::Msg(said) = event else {
                return;
            };
            // (Cancelled from the bar while it was written: its task is
            // gone already, and nothing is pasted.)
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if app.tasks.cancel(id) {
                app.tasks_changed();
            }
            match said {
                Ok(text) if text.trim().is_empty() => app.dictate_say("Nothing was made out"),
                Ok(text) => {
                    info!("dictation: {} characters written", text.chars().count());
                    app.serve_transient_text(text.trim());
                    app.after_ms(PASTE_AFTER_MS, |_| super::card_paste_key());
                }
                Err(why) => app.dictate_say(why),
            }
        });
        if waiting.is_err() {
            warn!("dictation: cannot wait for the words");
            if self.tasks.cancel(id) {
                self.tasks_changed();
            }
        }
        "writing".to_owned()
    }

    /// Say something on the bar for a moment (what went wrong).
    fn dictate_say(&mut self, why: &'static str) {
        info!("dictation: {why}");
        let (id, _) = self.tasks.begin(why);
        self.tasks.report(Report::Counts { id, counts: Counts::Words }, Instant::now());
        self.tasks_changed();
        self.after_ms(SAID_MS, move |app| {
            if app.tasks.cancel(id) {
                app.tasks_changed();
            }
        });
    }
}

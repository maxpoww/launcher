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
//! What is seen is THE CARD'S PILL on the bar, the small round one, which
//! BECOMES it (Max, the same day, in three looks; the last: *"we don't need
//! the seconds, leave only the mic, and when I let go I want one of those
//! semi rings spinning around the pill"*): its symbol is a microphone while
//! the key is down, and half a ring turns round its rim while the speech
//! engine works; then it is the card's pill again and the words are in the
//! window. Escape (a compositor bind, there only while it is written out)
//! or a click on the pill cancels. What goes wrong is said there, the pill
//! grown for the words, for a moment. (`clipboard.rs::push_clip_glyph` draws
//! it, from [`Show`].) Sound and engine are the card's own (`voice.rs`):
//! recorded and written out on this machine.

use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tracing::{info, warn};

use super::{voice, HOLD_MS};
use crate::App;

/// The key is down.
pub(super) struct Dictation {
    /// The recorder and what it writes — or why nothing is recorded.
    rec: Result<(Child, PathBuf), &'static str>,
    /// Since when: also WHICH press this is, for the timers that outlive it.
    since: Instant,
    /// It has been down long enough to be dictation (the pill says so).
    held: bool,
}

/// What the card's pill shows of a dictation.
#[derive(Debug, Clone)]
pub(crate) enum Show {
    /// Recording, since then.
    Listening(Instant),
    /// What was said is being written out, since then; and the way to stop
    /// that, if it is cancelled meanwhile.
    Writing(Instant, Arc<Halt>),
    /// Something went wrong: said for a moment (since then).
    Said(&'static str, Instant),
}

/// Stopping what is being written out: the flag its result is dropped by,
/// and the speech engine's process, which is KILLED — left to finish, a
/// cancelled sentence went on taking the processor for as long as it took
/// (twenty seconds on the old MacBook), under the next one.
#[derive(Debug, Default)]
pub(crate) struct Halt {
    halted: AtomicBool,
    engine: AtomicI32,
}

impl Halt {
    fn halt(&self) {
        self.halted.store(true, Ordering::Relaxed);
        let pid = self.engine.load(Ordering::Relaxed);
        if pid > 0 {
            // SAFETY: a signal to a child of ours that its thread still waits for.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }

    fn halted(&self) -> bool {
        self.halted.load(Ordering::Relaxed)
    }
}

/// How often the seconds are looked at, and a frame of what moves.
const TICK_MS: u64 = 200;
const FRAME_MS: u64 = 16;
/// How long the pill takes to grow to what it shows, and back.
const MORPH_MS: f32 = 200.0;
/// A recording ends by itself after this long: a key whose "up" never came
/// (the session locked meanwhile) does not record for ever.
const LONGEST_S: u64 = 180;
/// How long something that went wrong stays said on the bar.
const SAID_MS: u64 = 2600;
/// An "up" that arrives this soon before its own "down" was that tap's.
const CROSSED_MS: u128 = 400;
/// From the words being offered to the paste key: the selection is the
/// dock's before the window asks for it.
const PASTE_AFTER_MS: u64 = 80;

impl App {
    /// What the card's pill is to show of a dictation, if one is on.
    pub(crate) fn card_dictate_show(&self) -> Option<&Show> {
        self.card.dictate_show.as_ref()
    }

    /// How much wider than round the card's pill is NOW (px): easing from
    /// what it was to what its dictation asks for.
    pub(crate) fn card_dictate_extra(&self) -> f32 {
        let want = self.clip_dictate_extra(self.card.dictate_show.as_ref());
        let Some((from, at)) = self.card.dictate_from else {
            return want;
        };
        let k = (at.elapsed().as_secs_f32() * 1000.0 / MORPH_MS).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        from + (want - from) * k
    }

    /// The card's pill shows this now (`None`: it is the card's round pill
    /// again). Escape is taken for as long as something is written out.
    fn dictate_show(&mut self, show: Option<Show>) {
        self.card.dictate_from = Some((self.card_dictate_extra(), Instant::now()));
        // (Listening too, not only while it is written out: with Super still
        // down — it is, the key is Super+Space — Escape is Super+Escape, the
        // key that puts the computer to SLEEP. The config's bind for that
        // one asks `golem_dictating` first; found in review, 2026-10-13.)
        let on = matches!(show, Some(Show::Listening(_) | Show::Writing(..)));
        self.card.dictate_show = show;
        if on != self.card.dictate_esc {
            self.card.dictate_esc = on;
            self.dictate_esc(on);
        }
        self.sync_options_input();
        self.draw_options();
        self.dictate_frames();
    }

    /// ESCAPE cancels while what was said is written out. The dock has no
    /// keyboard then (the window keeps it, to be pasted into), so the key is
    /// a compositor bind for exactly that long — and `golem_dictating` tells
    /// the config, whose Super+Escape must not suspend the computer under a
    /// hand that has not let go of Super yet (hyprland.lua).
    ///
    /// Also called once as the dock starts (`main`), to give the key BACK: a dock
    /// that died while something was being written out left its bind in the
    /// compositor, and Escape was then eaten everywhere (and Super+Escape
    /// no longer put the computer to sleep) until the next dictation ended
    /// — shown in the rig, 2026-10-13, by killing the dock at that moment.
    pub(crate) fn dictate_esc(&self, on: bool) {
        let ctl = crate::hypr::ctl_program();
        let mut lua = format!("golem_dictating = {on} hl.unbind(\"ESCAPE\") ");
        if let (true, Some(ctl)) = (on, &ctl) {
            lua.push_str(&format!(
                "hl.bind(\"ESCAPE\", hl.dsp.exec_cmd(\"{} card dictate cancel\"))",
                ctl.display()
            ));
        }
        if !crate::hypr::eval_ok(&lua) {
            warn!("dictation: Escape could not be {}", if on { "taken" } else { "given back" });
        }
    }

    /// The key went down.
    pub(super) fn dictate_down(&mut self) -> String {
        // ("up" came FIRST: the two are two programs started a moment apart,
        // and on a quick tap the second can win. It was a tap.)
        self.card.dictate_up_owed = false;
        if self.card.dictate_up_at.take().is_some_and(|at| at.elapsed().as_millis() < CROSSED_MS) {
            return self.card_command("at voice");
        }
        // THE CARD IS UP HERE: the key is the card's. It turns to
        // voice-to-text's page and listens at once, as Space held on that
        // page does; let go, what was said is written by the card (Max,
        // 2026-10-12: "all of the keybinds should go to the card because
        // it's open").
        if self.card_up_here() && self.card.dictation.is_none() && self.card.talk_key.is_none() {
            // (Something is being recorded from the card's own buttons: the
            // key leaves it alone — a tap would have thrown it away.)
            if self.card.rec.is_some() {
                return "busy".to_owned();
            }
            let was_talk = self.card.page == super::Page::Talk;
            if !was_talk {
                self.card_show(super::Page::Talk);
            }
            self.card_record_start(true);
            self.card.talk_key = Some((Instant::now(), was_talk));
            return "down (the card)".to_owned();
        }
        // (Down already — a key that repeats — or the card is recording, or
        // the last thing said is still being written out.)
        if self.card.dictation.is_some()
            || self.card.rec.is_some()
            || matches!(self.card.dictate_show, Some(Show::Writing(..)))
        {
            return "busy".to_owned();
        }
        let since = Instant::now();
        self.card.dictation = Some(Dictation { rec: voice::recorder(true), since, held: false });
        self.after_ms(HOLD_MS, move |app| app.dictate_held(since));
        "down".to_owned()
    }

    /// Still down after `HOLD_MS`: it is dictation, and the pill says so.
    fn dictate_held(&mut self, since: Instant) {
        let Some(d) = self.card.dictation.as_mut().filter(|d| d.since == since) else {
            return;
        };
        if let Err(why) = d.rec {
            self.card.dictation = None;
            self.dictate_say(why);
            return;
        }
        d.held = true;
        info!("dictation: listening");
        self.dictate_show(Some(Show::Listening(since)));
        // (It starts with the pill's own bright beat, the one a muted
        // notification lands on the bell with: Max, 2026-10-12, "a bright
        // beating when the recording starts, like a spark".)
        self.trigger_clip_beat();
        self.dictate_tick(since);
    }

    /// While it listens: the seconds on the pill, and its limit.
    fn dictate_tick(&mut self, since: Instant) {
        self.after_ms(TICK_MS, move |app| {
            if !app.card.dictation.as_ref().is_some_and(|d| d.since == since && d.held) {
                return;
            }
            if since.elapsed().as_secs() >= LONGEST_S {
                // (Its "up" never came — the config was reloaded, the two
                // messages crossed: what it recorded is three minutes of the
                // room, and is thrown away, not pasted into a window.)
                info!("dictation: no key-up in {LONGEST_S} s — dropped");
                app.dictate_drop();
            } else {
                app.draw_options();
                app.dictate_tick(since);
            }
        });
    }

    /// Frames, for as long as something moves: the pill growing or going
    /// back, the dots while it is written out. One loop at a time.
    fn dictate_frames(&mut self) {
        if std::mem::replace(&mut self.card.dictate_ticking, true) {
            return;
        }
        self.after_ms(FRAME_MS, |app| {
            app.card.dictate_ticking = false;
            let morphing = app
                .card
                .dictate_from
                .is_some_and(|(_, at)| at.elapsed().as_secs_f32() * 1000.0 < MORPH_MS);
            if !morphing && app.card.dictate_from.take().is_some() {
                // (Arrived: the pill's place under the pointer is its new one.)
                app.sync_options_input();
            }
            app.draw_options();
            // (Listening it breathes, written out its ring turns.)
            if morphing || matches!(app.card.dictate_show, Some(Show::Listening(_) | Show::Writing(..))) {
                app.dictate_frames();
            }
        });
    }

    /// A click on the card's pill while it shows a dictation: that is
    /// cancelled — nothing is written. Whether there was one.
    pub(crate) fn dictate_cancel(&mut self) -> bool {
        match self.card.dictate_show.clone() {
            Some(Show::Listening(_)) => {
                info!("dictation: cancelled from the bar");
                self.dictate_drop();
            }
            Some(Show::Writing(_, halt)) => {
                info!("dictation: cancelled from the bar");
                halt.halt();
                self.dictate_show(None);
            }
            Some(Show::Said(..)) => self.dictate_show(None),
            None => return false,
        }
        true
    }

    /// Throw away what is being recorded.
    fn dictate_drop(&mut self) {
        let Some(d) = self.card.dictation.take() else {
            return;
        };
        // (Its key is still down, unless this is the tap's own drop.)
        self.card.dictate_up_owed = d.held;
        if let Ok((mut child, path)) = d.rec {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
                let _ = std::fs::remove_file(path);
            });
        }
        if d.held {
            self.dictate_show(None);
        }
    }

    /// The key came up. A tap: the card, at voice-to-text. Held: what was
    /// said is written out, then pasted into the window.
    pub(super) fn dictate_up(&mut self) -> String {
        // (A talk key whose "up" was lost long ago is nobody's now.)
        let talk_key = self.card.talk_key.take().filter(|(since, _)| since.elapsed().as_secs() < LONGEST_S);
        if let Some((since, was_talk)) = talk_key {
            if since.elapsed().as_millis() as u64 >= HOLD_MS {
                self.card_record_stop(true);
                return "writing (the card)".to_owned();
            }
            // (A tap: nothing was said. On voice-to-text's page already, it
            // is the same key again — the card goes away.)
            self.card_record_stop(false);
            return if was_talk { self.card_command("at voice") } else { "at voice".to_owned() };
        }
        let Some(held) = self.card.dictation.as_ref().map(|d| d.held) else {
            // (The "up" of a dictation that was cancelled, or dropped, while
            // the key was still down is that one's: it announces no tap.)
            if !std::mem::take(&mut self.card.dictate_up_owed) {
                self.card.dictate_up_at = Some(Instant::now());
            }
            return "not down".to_owned();
        };
        if !held {
            self.dictate_drop();
            return self.card_command("at voice");
        }
        let Some(Dictation { rec: Ok((mut child, path)), .. }) = self.card.dictation.take() else {
            return "nothing recorded".to_owned();
        };
        // (Interrupted, not killed: the recorder closes its file whole.)
        // SAFETY: a signal to a child of ours that is still to be waited for.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGINT);
        }
        let (since, halt) = (Instant::now(), Arc::new(Halt::default()));
        self.dictate_show(Some(Show::Writing(since, halt.clone())));
        let engine_of = halt.clone();
        let engine = voice::engine();
        let (tx, rx) = calloop::channel::channel::<Result<String, &'static str>>();
        std::thread::spawn(move || {
            let _ = child.wait();
            let said = match voice::heard(&path, true) {
                voice::Heard::Nothing => Err("Nothing was heard. Is the microphone muted or off?"),
                voice::Heard::TooLoud(hiss) => Err(voice::mic_turn_down(hiss)),
                voice::Heard::Sound => Ok(()),
            };
            let said = if let Err(why) = said {
                Err(why)
            } else if engine_of.halted() {
                Err("")
            } else if let Some((cli, model)) = engine {
                voice::transcribe(&cli, &model, &path, Some(&engine_of.engine))
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
            // (Cancelled from the bar while it was written: the pill has
            // gone back already, and nothing is pasted.)
            if halt.halted() {
                return;
            }
            match said {
                Ok(text) if text.trim().is_empty() => app.dictate_say("Nothing was made out"),
                Ok(text) => {
                    info!("dictation: {} characters written", text.chars().count());
                    app.dictate_show(None);
                    app.serve_transient_text(text.trim());
                    app.after_ms(PASTE_AFTER_MS, |_| super::card_paste_key());
                }
                Err(why) => app.dictate_say(why),
            }
        });
        if waiting.is_err() {
            warn!("dictation: cannot wait for the words");
            self.dictate_show(None);
        }
        "writing".to_owned()
    }

    /// Say something on the pill for a moment (what went wrong).
    fn dictate_say(&mut self, why: &'static str) {
        info!("dictation: {why}");
        // (The pill is a line of some thirty letters: the card's sentences,
        // which have a whole line under its box, are said shorter here.)
        let why = match why {
            w if w.starts_with("Nothing was heard") => "Nothing was heard",
            w if w.starts_with("The microphone is muted") => "The microphone is muted",
            w if w.starts_with("The microphone was too loud") => "Mic was too loud: say it again",
            w if w.starts_with("Too much noise") => "Too much noise",
            w if w.contains("speech engine") => "No speech engine installed",
            w => w,
        };
        let since = Instant::now();
        self.dictate_show(Some(Show::Said(why, since)));
        self.after_ms(SAID_MS, move |app| {
            if matches!(app.card.dictate_show, Some(Show::Said(_, at)) if at == since) {
                app.dictate_show(None);
            }
        });
    }
}

//! SCREEN RECORDING (the plan's F3.3): the screen the keyboard is on,
//! recorded into Videos → Screencasts until it is stopped.
//!
//! One verb, `waverunner-ctl record` (Golem binds it to Ctrl+Print, beside
//! the screenshot keys): the first press starts, the next one stops. While it
//! runs it is a TASK on the bar's task pill — "Recording the screen", how
//! long it has run where a percentage would be, and a `Stop` chip (the task
//! pill's Cancel under the word that is true here: stopping KEEPS the
//! recording). No pill of its own: the task pill is where long things show.
//!
//! The recorder is `wf-recorder` (Golem's home layer brings it;
//! `WAVERUNNER_WF_RECORDER` names another), one whole screen, no sound. It is
//! ended with SIGINT, which is how it closes its file properly, and it ends
//! with the dock (PDEATHSIG, as the card's voice recorder does): a recorder
//! left alone would fill the disk with a film nobody asked for.
//!
//! When it is over a notification says where the film is; its click opens
//! the folder. A recorder that ends by itself (no screen to capture, no
//! encoder) is said too, and the empty file is not left behind.
//!
//! Pointer-free: `record [toggle|start|stop|state]`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use tracing::{info, warn};

use crate::i18n::{fill, tr};
use crate::tasks::TaskHandle;
use crate::App;

/// A film shorter than this many bytes is not a film (a header, or nothing).
const EMPTY: u64 = 2048;
/// How long the recorder gets to close its file before it is killed.
const CLOSE_WITHIN: Duration = Duration::from_secs(8);

/// A recording that is running.
pub(crate) struct Recording {
    child: Child,
    file: PathBuf,
    since: Instant,
    task: TaskHandle,
}

/// The recorder's program.
fn recorder() -> PathBuf {
    std::env::var_os("WAVERUNNER_WF_RECORDER").map_or_else(|| PathBuf::from("wf-recorder"), PathBuf::from)
}

/// Videos → Screencasts, in the owner's own Videos folder.
fn folder() -> PathBuf {
    let home = crate::filer::home();
    let conf = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let text = std::fs::read_to_string(conf.join("user-dirs.dirs")).unwrap_or_default();
    let videos = crate::filer::user_dir(&text, &home, "VIDEOS").filter(|d| *d != home).unwrap_or_else(|| home.join("Videos"));
    videos.join("Screencasts")
}

/// `Screencast 2026-11-16 14-03-21.mp4`: the screenshots' way of naming.
fn file_name(at: SystemTime) -> String {
    let secs = at.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: `localtime_r` fills a caller-owned `tm` from a valid `time_t`.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    format!(
        "Screencast {:04}-{:02}-{:02} {:02}-{:02}-{:02}.mp4",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// How long it has run, as the pill says it: `0:07`, `12:40`, `1:02:03`.
pub(crate) fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The recorder's command for `screen` into `file`.
fn command(screen: &str, file: &Path) -> Command {
    let mut cmd = Command::new(recorder());
    // -y: never ask on its input; yuv420p: a film every player opens.
    cmd.arg("-y").arg("-o").arg(screen).arg("-x").arg("yuv420p").arg("-f").arg(file);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: `prctl` is async-signal-safe and touches nothing of ours.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // (As a full word: see the card's voice recorder.)
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGINT as libc::c_ulong);
            Ok(())
        });
    }
    cmd
}

/// Say how a recording ended.
fn say_saved(file: &Path) {
    let folder = file.parent().map(|p| p.display().to_string()).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let show = format!("{}{folder}", crate::NOTIFY_OPEN);
    crate::desktop_send_notify_open(tr("Screen recording"), "video-x-generic", &fill(tr("Saved as {name}"), &[("name", &name)]), Some(&show));
}

fn say_failed() {
    crate::desktop_send_notify_open(tr("Screen recording"), "video-x-generic", tr("The screen could not be recorded"), None);
}

/// Whether there is a film in `file`; an empty one is taken away.
fn kept(file: &Path) -> bool {
    let size = std::fs::metadata(file).map_or(0, |m| m.len());
    if size < EMPTY {
        let _ = std::fs::remove_file(file);
        return false;
    }
    true
}

impl App {
    /// `waverunner-ctl record [toggle|start|stop|state]`.
    pub(crate) fn record_command(&mut self, what: &str) -> String {
        match what.trim() {
            "" | "toggle" => {
                if self.recording.is_some() {
                    self.record_stop();
                } else {
                    self.record_start();
                }
            }
            "start" => {
                if self.recording.is_none() {
                    self.record_start();
                }
            }
            "stop" => self.record_stop(),
            "state" => {}
            _ => return "record [toggle|start|stop|state]".to_owned(),
        }
        match &self.recording {
            Some(r) => format!("recording {} ({})", r.file.display(), clock(r.since.elapsed().as_secs())),
            None => "not recording".to_owned(),
        }
    }

    fn record_start(&mut self) {
        let screen = match crate::hypr::focused_monitor() {
            Ok(m) => m.name,
            Err(e) => {
                warn!("record: no screen to record ({e:#})");
                say_failed();
                return;
            }
        };
        let dir = folder();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("record: cannot make {dir:?}: {e}");
            say_failed();
            return;
        }
        let file = dir.join(file_name(SystemTime::now()));
        let child = match command(&screen, &file).spawn() {
            Ok(child) => child,
            Err(e) => {
                warn!("record: the recorder did not start ({:?}): {e}", recorder());
                say_failed();
                return;
            }
        };
        info!("record: recording {screen} into {file:?}");
        let task = self.task_begin_with(tr("Recording the screen"), Some(clock(0)));
        self.tasks.keeps(task.id());
        self.tasks_changed();
        self.recording = Some(Recording { child, file, since: Instant::now(), task });
        self.record_tick_arm();
    }

    /// Once a second while it runs: the time on the pill, and a recorder
    /// that ended by itself noticed.
    fn record_tick_arm(&mut self) {
        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
        let armed = self.loop_handle.insert_source(Timer::from_duration(Duration::from_secs(1)), |_, _, app: &mut App| {
            let Some(rec) = app.recording.as_mut() else {
                return TimeoutAction::Drop;
            };
            match rec.child.try_wait() {
                Ok(None) => {
                    rec.task.number(&clock(rec.since.elapsed().as_secs()));
                    TimeoutAction::ToDuration(Duration::from_secs(1))
                }
                ended => {
                    warn!("record: the recorder ended by itself ({ended:?})");
                    if let Some(rec) = app.recording.take() {
                        // (What it made before it ended is the owner's.)
                        if kept(&rec.file) {
                            say_saved(&rec.file);
                        } else {
                            say_failed();
                        }
                    }
                    TimeoutAction::Drop
                }
            }
        });
        if let Err(e) = armed {
            warn!("record: cannot count the recording's time ({e})");
        }
    }

    /// Stop and keep: the recorder closes its file off the loop, then the
    /// owner is told where the film is.
    fn record_stop(&mut self) {
        let Some(Recording { mut child, file, since, task }) = self.recording.take() else {
            return;
        };
        info!("record: stopped after {} s", since.elapsed().as_secs());
        // (The task is over now: the pill fills and leaves.)
        drop(task);
        // SAFETY: a signal to a child of ours, by its pid.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGINT);
        }
        let closing = std::thread::Builder::new().name("record-close".into()).spawn(move || {
            let until = Instant::now() + CLOSE_WITHIN;
            loop {
                match child.try_wait() {
                    Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(100)),
                    Ok(None) => {
                        warn!("record: the recorder did not close its file in time");
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => break,
                }
            }
            if kept(&file) {
                say_saved(&file);
            } else {
                say_failed();
            }
        });
        if let Err(e) = closing {
            warn!("record: cannot wait for the recorder to close ({e})");
        }
    }

    /// The task pill's Stop on the recording's task.
    pub(crate) fn record_task_cancelled(&mut self, task: u64) {
        if self.recording.as_ref().is_some_and(|r| r.task.id() == task) {
            self.record_stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_reads_as_a_clock() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(7), "0:07");
        assert_eq!(clock(760), "12:40");
        assert_eq!(clock(3723), "1:02:03");
    }

    #[test]
    fn a_film_is_named_as_the_screenshots_are() {
        let name = file_name(SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_800_000));
        assert!(name.starts_with("Screencast 20") && name.ends_with(".mp4"), "{name}");
        assert_eq!(name.len(), "Screencast 2026-11-16 14-03-21.mp4".len());
        assert!(!name.contains(':'), "no colon: a stick's filesystem refuses it");
    }

    #[test]
    fn an_empty_film_is_not_left_behind() {
        let dir = std::env::temp_dir().join(format!("wr-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (empty, film) = (dir.join("empty.mp4"), dir.join("film.mp4"));
        std::fs::write(&empty, b"").unwrap();
        std::fs::write(&film, vec![0u8; 4096]).unwrap();
        assert!(!kept(&empty) && !empty.exists());
        assert!(kept(&film) && film.exists());
        assert!(!kept(&dir.join("never-made.mp4")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_recorder_is_asked_for_one_screen_and_never_asks_back() {
        let cmd = command("HDMI-A-1", Path::new("/tmp/x.mp4"));
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-y", "-o", "HDMI-A-1", "-x", "yuv420p", "-f", "/tmp/x.mp4"]);
    }
}

//! YOUR ACCOUNT (Max, 2026-11-06, the Controls gap audit: "contraseña",
//! "usuario", "huella" — the first thing people look for and nothing
//! answered; the password could only be changed in a terminal): Controls →
//! YOUR ACCOUNT — who you are, your picture, the password changed, a
//! fingerprint registered where there is a reader.
//!
//! THE PASSWORD is changed by `passwd` itself (the system's own rules, PAM,
//! the seed file Golem keeps beside `/etc/shadow` — `base/users.nix`), run
//! on a pseudo-terminal of ours ([`run_on_tty`]): `passwd` asks on the
//! terminal, never on its input, so a pipe would not do. The three fields
//! are typed in the panel (the Colours editor's field machinery), the words
//! never leave this process but into `passwd`, and what it answered is said
//! under the button ("Password changed", or its own reason). THE PICTURE is
//! `~/.face`, the freedesktop place every greeter and chat app reads: a
//! picture picked through the portal, made square, 256 px, PNG. THE
//! FINGERPRINT is fprintd's (`fprintd-enroll`, on a thread: "touch the
//! reader" until it is done; `fprintd-list` says how many are registered,
//! `fprintd-delete` forgets them). Where the system has no fprintd there is
//! no row.

use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::controls::{choice_of, field_of, group_of, line_of, Choice, ControlView, Group};
use crate::i18n::{fill, tr};

/// The three fields' texts while the page is up (typed into, never saved).
static TYPED: Mutex<[String; 3]> = Mutex::new([String::new(), String::new(), String::new()]);

/// What the last change said, under the button.
static SAID: Mutex<Option<(bool, String)>> = Mutex::new(None);

/// A finger is being registered; a picture is being chosen.
static ENROLLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const FIELDS: [&str; 3] = ["account-current", "account-new", "account-again"];

fn field_index(key: &str) -> Option<usize> {
    FIELDS.iter().position(|k| *k == key)
}

/// The field's text as the panel asks for it (its key, what it starts with).
pub(crate) fn field_start(key: &str) -> Option<(&'static str, String)> {
    let i = field_index(key)?;
    Some((FIELDS[i], TYPED.lock().ok()?[i].clone()))
}

/// What was typed, kept at Enter (and as the click goes elsewhere).
pub(crate) fn field_typed(key: &str, text: &str) {
    if let (Some(i), Ok(mut t)) = (field_index(key), TYPED.lock()) {
        t[i] = text.to_owned();
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub(crate) fn face_path() -> PathBuf {
    home().join(".face")
}

/// The user's name and full name, from the account database.
pub(crate) fn whoami() -> (String, String) {
    let user = std::env::var("USER").unwrap_or_default();
    let full = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| {
            p.lines().find_map(|l| {
                let mut f = l.split(':');
                (f.next()? == user).then(|| f.nth(3).unwrap_or("").split(',').next().unwrap_or("").trim().to_owned())
            })
        })
        .unwrap_or_default();
    (user, full)
}

/// How many fingers are registered; `None` without fprintd or a reader.
fn fingers() -> Option<usize> {
    if !crate::launch::on_path("fprintd-list") {
        return None;
    }
    let user = std::env::var("USER").unwrap_or_default();
    let out = crate::controls::out_of("fprintd-list", &[&user])?;
    if out.contains("No devices available") || out.contains("no devices") {
        return None;
    }
    Some(out.lines().filter(|l| l.trim_start().starts_with("- #")).count())
}

/// Controls → Your account.
pub(crate) fn view(view: &mut ControlView) {
    let (user, full) = whoami();
    view.subtitle = if full.is_empty() || full == user { user.clone() } else { format!("{full} · {user}") };
    // THE PICTURE: shown as a card when there is one, chosen through the portal.
    let face = face_path();
    if face.is_file() {
        let tile = Choice { caption: true, ..choice_of(tr("Your picture"), false, face.to_string_lossy().into_owned()) };
        view.groups.push(Group { tiles: true, grid_rows: Some(1), ..group_of(None, "account-face", vec![tile]) });
    }
    let picking = PICKING.load(std::sync::atomic::Ordering::Relaxed);
    let word = if picking { tr("Choosing…") } else if face.is_file() { tr("Change the picture…") } else { tr("Choose a picture…") };
    view.groups.push(Group { buttons: true, ..group_of(None, "account-photo", vec![choice_of(word, false, "pick")]) });
    // THE PASSWORD: three fields in a row, the button, what it said.
    let typed = TYPED.lock().map(|t| t.clone()).unwrap_or_default();
    let dots = |s: &str| "•".repeat(s.chars().count());
    let mut row = field_of("account-current", &dots(&typed[0]), tr("Current password"), 180.0);
    let new = field_of("account-new", &dots(&typed[1]), tr("New password"), 180.0);
    let again = field_of("account-again", &dots(&typed[2]), tr("New password again"), 180.0);
    for (f, key) in [(new, "account-new"), (again, "account-again")] {
        row.choices.push(Choice { value: key.to_owned(), ..f.choices[0].clone() });
    }
    row.key = "account-fields";
    row.width = 180.0;
    view.groups.push(row);
    view.groups.push(Group { buttons: true, ..group_of(None, "account-change", vec![choice_of(tr("Change the password"), false, "change")]) });
    if let Some((ok, said)) = SAID.lock().ok().and_then(|s| s.clone()) {
        let text = if ok { said } else { fill(tr("Not changed: {why}"), &[("why", &said)]) };
        view.groups.push(Group { centred: true, ..line_of(text) });
    }
    // THE FINGERPRINT, where there is a reader.
    if let Some(n) = fingers() {
        let line = match n {
            0 => tr("No fingerprint registered.").to_owned(),
            1 => tr("One fingerprint registered.").to_owned(),
            n => fill(tr("{n} fingerprints registered."), &[("n", &n)]),
        };
        view.groups.push(Group { centred: true, ..line_of(line) });
        let enrolling = ENROLLING.load(std::sync::atomic::Ordering::Relaxed);
        let mut buttons = vec![choice_of(if enrolling { tr("Touch the reader…") } else { tr("Register a finger") }, false, "enroll")];
        if n > 0 && !enrolling {
            buttons.push(choice_of(tr("Forget the fingerprints"), false, "forget"));
        }
        view.groups.push(Group { buttons: true, ..group_of(None, "account-finger", buttons) });
    }
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "account-change" => change_password(),
        "account-photo" => pick_picture(),
        "account-finger" if value == "enroll" => enroll(),
        "account-finger" if value == "forget" => {
            let user = std::env::var("USER").unwrap_or_default();
            crate::controls::run_of("fprintd-delete", &[&user]);
        }
        "account-fields" | "account-face" => {}
        _ => return false,
    }
    true
}

/// `passwd` with the three fields: a reason under the button either way.
fn change_password() {
    let typed = TYPED.lock().map(|t| t.clone()).unwrap_or_default();
    let said = if typed[1].is_empty() {
        Err(tr("type the new password").to_owned())
    } else if typed[1] != typed[2] {
        Err(tr("the two new passwords differ").to_owned())
    } else if typed[1].chars().count() < 8 {
        Err(tr("a password has 8 characters or more").to_owned())
    } else {
        passwd(&typed[0], &typed[1])
    };
    if let Ok(mut t) = TYPED.lock() {
        // (The current one is kept only while it failed: a slip is retyped.)
        let keep = said.is_err();
        if !keep {
            t[0].clear();
        }
        t[1].clear();
        t[2].clear();
    }
    if let Ok(mut s) = SAID.lock() {
        *s = Some(match said {
            Ok(()) => (true, tr("Password changed.").to_owned()),
            Err(why) => (false, why),
        });
    }
}

/// Run `passwd` on a terminal of ours and answer its prompts.
fn passwd(current: &str, new: &str) -> Result<(), String> {
    let answers = [("current", current), ("old", current), ("new", new), ("retype", new), ("again", new), ("password:", current)];
    let (status, out) = run_on_tty("passwd", &[], &answers, Duration::from_secs(20))?;
    info!("account: passwd ended {status:?}");
    if status == Some(0) {
        return Ok(());
    }
    // Its own reason, in its own words: the last line that says something.
    let why = out
        .lines()
        .map(|l| l.trim().trim_start_matches("passwd:").trim())
        .rfind(|l| {
            let low = l.to_lowercase();
            !l.is_empty() && !low.ends_with("password:") && !low.ends_with("password: ") && !low.starts_with("changing password")
        })
        .unwrap_or("")
        .to_owned();
    Err(if why.is_empty() { tr("passwd refused it").to_owned() } else { why })
}

/// Run a program on a pseudo-terminal, answering its prompts: each
/// `(word, answer)` of `answers` sends the answer (and Enter) the first
/// time a line ending the prompt contains the word (case-blind), at most
/// once per prompt; everything the program printed comes back with its
/// exit status (`None`: ended by us at `patience`).
pub(crate) fn run_on_tty(program: &str, args: &[&str], answers: &[(&str, &str)], patience: Duration) -> Result<(Option<i32>, String), String> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // SAFETY: openpty fills the two descriptors; the pointers are to our own ints.
    let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) };
    if r != 0 {
        return Err(tr("no terminal could be made").to_owned());
    }
    // SAFETY: both are fresh descriptors openpty gave us.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?)).stdout(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?)).stderr(Stdio::from(slave));
    cmd.env("LC_ALL", "C");
    // SAFETY: setsid and the controlling-terminal ioctl, in the child before exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    let mut file = std::fs::File::from(master);
    // (A read with nothing to read must not hold us: the descriptor is
    // non-blocking and polled.)
    // SAFETY: fcntl on our own descriptor.
    unsafe {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
    }
    let start = Instant::now();
    let mut out = String::new();
    let mut answered = vec![false; answers.len()];
    let mut line = String::new();
    let mut status = None;
    let mut buf = [0u8; 512];
    loop {
        match file.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                out.push_str(&text);
                line.push_str(&text);
                if let Some(cut) = line.rfind('\n') {
                    line = line[cut + 1..].to_owned();
                }
                let prompt = line.trim().to_lowercase();
                if !prompt.is_empty() {
                    if let Some(i) = answers.iter().enumerate().position(|(i, (word, _))| !answered[i] && prompt.contains(word)) {
                        answered[i] = true;
                        let _ = file.write_all(format!("{}\n", answers[i].1).as_bytes());
                        line.clear();
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            // (The other end closed: the program ended.)
            Err(_) => {}
        }
        match child.try_wait() {
            Ok(Some(s)) => {
                status = s.code();
                // The last words, if any came after the exit was seen.
                let mut rest = String::new();
                let _ = file.read_to_string(&mut rest);
                out.push_str(&rest);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if start.elapsed() > patience {
            warn!("account: {program} took too long; ended");
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok((status, out))
}

/// The portal's picker, on a thread of its own; the picture made square
/// and small, written as `~/.face`; the page read again.
fn pick_picture() {
    if PICKING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new().name("account-picture".into()).spawn(|| {
        if let Some(path) = crate::bt_files::pick_file_waiting() {
            match image::open(&path) {
                Ok(img) => {
                    let img = img.to_rgba8();
                    let (w, h) = (img.width(), img.height());
                    let side = w.min(h);
                    let cropped = image::imageops::crop_imm(&img, (w - side) / 2, (h - side) / 2, side, side).to_image();
                    let small = image::imageops::resize(&cropped, 256, 256, image::imageops::FilterType::Lanczos3);
                    let face = face_path();
                    let tmp = face.with_extension("tmp.png");
                    match small.save_with_format(&tmp, image::ImageFormat::Png).and_then(|_| std::fs::rename(&tmp, &face).map_err(Into::into)) {
                        Ok(()) => info!("account: picture {} → {}", path.display(), face.display()),
                        Err(e) => warn!("account: could not keep the picture: {e}"),
                    }
                }
                Err(e) => warn!("account: {} is not a picture: {e}", path.display()),
            }
        }
        PICKING.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::controls::read_again("Your account");
    });
    if spawned.is_err() {
        PICKING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `fprintd-enroll` on a thread: the reader is touched until it is done.
fn enroll() {
    if ENROLLING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new().name("account-finger".into()).spawn(|| {
        let user = std::env::var("USER").unwrap_or_default();
        let ok = crate::controls::finish_within(
            {
                let mut c = Command::new("fprintd-enroll");
                c.args(["-f", "right-index-finger", &user]);
                c
            },
            true,
            Duration::from_secs(90),
        );
        info!("account: enroll ended: {}", ok.as_deref().unwrap_or("(nothing said)").trim());
        ENROLLING.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::controls::read_again("Your account");
    });
    if spawned.is_err() {
        ENROLLING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_on_the_terminal_is_answered_at_its_prompt() {
        // `sh` reads a line on the terminal and echoes it back.
        let script = "printf 'Say it: '; read x; echo \"got $x\"; exit 3";
        let (status, out) = run_on_tty("sh", &["-c", script], &[("say it", "hello")], Duration::from_secs(5)).unwrap();
        assert_eq!(status, Some(3), "{out}");
        assert!(out.contains("got hello"), "{out}");
    }

    #[test]
    fn a_program_that_never_ends_is_ended() {
        let t = Instant::now();
        let (status, _) = run_on_tty("sh", &["-c", "sleep 30"], &[], Duration::from_millis(300)).unwrap();
        assert_eq!(status, None);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_fields_keep_what_is_typed() {
        field_typed("account-new", "secret");
        assert_eq!(field_start("account-new"), Some(("account-new", "secret".to_owned())));
        assert_eq!(field_start("account-nope"), None);
        field_typed("account-new", "");
    }
}

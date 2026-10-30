//! SCANNERS: a scanner found → an app to scan with, once (Max, 2026-10-21:
//! *"que Golem lo instale automáticamente cuando detecte un scanner"*).
//!
//! Golem's system side makes scanners work (desktop/printing.nix: SANE, its
//! own drivers, sane-airscan for the driverless eSCL/WSD ones, HP's), but the
//! debloat of 2026-09-29 took the scanning app out of the base system. So the
//! dock watches for a scanner and, the first time one is there and no
//! scanning app is installed, installs Simple Scan through the ordinary
//! install path (`start_managed_install`: one rebuild, the dock's own tile)
//! and says so. Once — offered or not needed — it is never offered again
//! (`OFFERED`, in the data folder): an owner who takes the app out is not
//! given it back.
//!
//! THE SCANNERS THERE ARE (2026-10-22, the Printers page's Scanners): the
//! watcher keeps looking and keeps what it found ([`known`]); the page reads
//! that, and asks for a fresh look ([`look_now`]) when what it has is old. A
//! look whose answer differs from the last is told to the loop, which reads
//! the page again.
//!
//! WHEN it looks (a worker thread, asleep otherwise): a while after the dock
//! starts (a scanner on the network, or plugged in before), whenever a new
//! USB device appears (`usb_watch`: the kernel says; where it cannot, sysfs
//! every `TICK`), and every `NETWORK` for one that
//! comes onto the network later. HOW: `scanimage -L`, SANE's own list — the
//! same backends every scanning app uses, so what it finds an app finds. A
//! webcam is not a scanner (SANE's v4l backend lists them; and Golem's phone
//! camera is one), nor SANE's virtual test device — unless
//! `WAVERUNNER_SCANNER_TEST` is set, which is how this is tested with no
//! scanner at hand (a SANE config with `test` in its dll.conf).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::App;

/// The marker that it has been done (the app offered, or one already there).
const OFFERED: &str = "scanner-app";
/// How often the USB bus is looked at where the kernel cannot say (sysfs
/// only: a directory listing).
const TICK: Duration = Duration::from_secs(3);
/// After the dock starts, before the first look.
const FIRST: Duration = Duration::from_secs(90);
/// A device just plugged in is given this long before SANE is asked.
const SETTLE: Duration = Duration::from_secs(4);
/// A look for scanners that come onto the network later.
const NETWORK: Duration = Duration::from_secs(6 * 3600);
/// `scanimage -L` is ended after this (a network backend can wait long).
const PATIENCE: Duration = Duration::from_secs(40);

/// The apps that scan: one of these installed and nothing is done.
const SCAN_APPS: [&str; 8] = [
    "org.gnome.SimpleScan",
    "simple-scan",
    "gscan2pdf",
    "xsane",
    "org.kde.skanlite",
    "org.kde.skanpage",
    "com.github.ruippeixotog.NAPS2",
    "naps2",
];
/// What is installed: Simple Scan (nixpkgs attr, its desktop id).
pub(crate) const APP_ATTR: &str = "simple-scan";
pub(crate) const APP_ID: &str = "org.gnome.SimpleScan.desktop";
/// The scanning apps by the program each is run as, for a page that opens
/// one (Simple Scan first: it takes the scanner to use as its argument).
pub(crate) const SCAN_PROGRAMS: [&str; 6] = ["simple-scan", "skanpage", "skanlite", "gscan2pdf", "naps2", "xsane"];

/// What the last look found, and when.
static SEEN: Mutex<Option<(Vec<Scanner>, Instant)>> = Mutex::new(None);
/// A look is wanted now (the Printers page found what it has old).
static LOOK: AtomicBool = AtomicBool::new(false);
/// Wakes the watcher for it (and for a USB device, from `usb_watch`).
static WAKE: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

/// The scanners the last look found, and how long ago; `None` before the
/// first look (or with no SANE on this machine).
pub(crate) fn known() -> Option<(Vec<Scanner>, Duration)> {
    SEEN.lock().ok()?.as_ref().map(|(s, at)| (s.clone(), at.elapsed()))
}

/// Ask the watcher for a look at its next tick.
pub(crate) fn look_now() {
    LOOK.store(true, Ordering::Relaxed);
    if let Some(wake) = WAKE.lock().ok().and_then(|w| w.clone()) {
        let _ = wake.send(());
    }
}

/// A scanner SANE lists: its device name and what it says it is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scanner {
    pub device: String,
    pub what: String,
}

/// The scanners in `scanimage -L`'s output (`device `X' is a Canon … scanner`).
pub(crate) fn parse_list(out: &str, allow_test: bool) -> Vec<Scanner> {
    out.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("device `")?;
            let (device, what) = rest.split_once("' is a ")?;
            Some(Scanner { device: device.to_owned(), what: what.trim().to_owned() })
        })
        .filter(|s| !s.device.starts_with("v4l:"))
        .filter(|s| allow_test || !s.device.starts_with("test:"))
        .collect()
}

/// The USB devices there are now (`1-2`, not their interfaces).
fn usb_devices(sysfs: &Path) -> BTreeSet<String> {
    std::fs::read_dir(sysfs)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.contains(':') && !n.starts_with("usb"))
                .collect()
        })
        .unwrap_or_default()
}

/// Ask SANE for its scanners, giving up after `PATIENCE`.
fn ask_sane(allow_test: bool) -> Vec<Scanner> {
    let Ok(mut child) = Command::new("scanimage")
        .arg("-L")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < PATIENCE => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
    }
    parse_list(&out, allow_test)
}

/// Whether it has been done for good.
fn offered() -> bool {
    crate::persist::data_path(OFFERED).exists()
}

/// Start watching, on a thread (with SANE on this machine): each look that
/// finds other scanners than the last is sent.
pub(crate) fn spawn(tx: calloop::channel::Sender<Vec<Scanner>>) {
    if !crate::launch::on_path("scanimage") {
        return;
    }
    let allow_test = std::env::var_os("WAVERUNNER_SCANNER_TEST").is_some();
    let first = std::env::var("WAVERUNNER_SCANNER_FIRST")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(FIRST);
    let spawned = std::thread::Builder::new().name("scanners".into()).spawn(move || {
        let sysfs = Path::new("/sys/bus/usb/devices");
        let mut known = usb_devices(sysfs);
        let mut due = Some(Instant::now() + first);
        let mut last_look = Instant::now();
        let (wake, woken) = std::sync::mpsc::channel::<()>();
        if let Ok(mut kept) = WAKE.lock() {
            *kept = Some(wake.clone());
        }
        let kernel_tells = crate::usb_watch::listen(move || wake.send(()).is_ok());
        loop {
            // Asleep until the next thing it has to do: only where the kernel
            // cannot say what was plugged in does it wake to look.
            let until_next = due
                .map_or(NETWORK.saturating_sub(last_look.elapsed()), |at| {
                    at.saturating_duration_since(Instant::now())
                })
                .max(Duration::from_millis(100));
            let _ = woken.recv_timeout(if kernel_tells { until_next } else { until_next.min(TICK) });
            let now = usb_devices(sysfs);
            if now.difference(&known).next().is_some() {
                due = Some(Instant::now() + SETTLE);
            }
            known = now;
            if last_look.elapsed() >= NETWORK || LOOK.swap(false, Ordering::Relaxed) {
                due = Some(Instant::now());
            }
            if due.is_some_and(|at| Instant::now() >= at) {
                due = None;
                last_look = Instant::now();
                let found = ask_sane(allow_test);
                let changed = SEEN.lock().map_or(true, |mut seen| {
                    let changed = seen.as_ref().is_none_or(|(was, _)| *was != found);
                    *seen = Some((found.clone(), Instant::now()));
                    changed
                });
                if changed && tx.send(found).is_err() {
                    return;
                }
            }
        }
    });
    if let Err(e) = spawned {
        warn!("scanners: cannot watch for one: {e}");
    }
}

impl App {
    /// A look found other scanners than the last: the Printers page shows
    /// them, and the first one ever brings an app to scan with.
    pub(crate) fn on_scanners(&mut self, found: Vec<Scanner>) {
        if let Some(first) = found.first().filter(|_| !offered()) {
            self.on_scanner(first.clone());
        }
        self.printers_page_follow();
    }

    /// A scanner is there: an app to scan with, if there is none — once.
    fn on_scanner(&mut self, found: Scanner) {
        let had = self
            .entries
            .iter()
            .find(|e| SCAN_APPS.iter().any(|id| e.id.trim_end_matches(".desktop") == *id))
            .map(|e| e.name.clone());
        let mark = crate::persist::data_path(OFFERED);
        if let Some(parent) = mark.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&mark, format!("{}\t{}\n", found.device, found.what));
        if let Some(app) = had {
            info!("scanners: {} found; {app} is there to scan with", found.what);
            return;
        }
        info!("scanners: {} found; installing {APP_ATTR}", found.what);
        self.start_managed_install(APP_ATTR, vec![APP_ID.to_owned()]);
        notify(
            crate::i18n::tr("Scanner found"),
            &crate::i18n::fill(
                crate::i18n::tr("{what}: installing Simple Scan to scan with."),
                &[("what", &found.what)],
            ),
        );
    }
}

/// A desktop notification through the session bus (as the battery's), on
/// a thread that waits for it (no process left behind).
fn notify(title: &str, body: &str) {
    let (title, body) = (title.to_owned(), body.to_owned());
    let spawned = std::thread::Builder::new().name("scan-notice".into()).spawn(move || notify_now(&title, &body));
    if let Err(e) = spawned {
        warn!("scanners: no thread for a notice: {e}");
    }
}

fn notify_now(title: &str, body: &str) {
    let r = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            "susssasa{sv}i",
            "waverunner",
            "0",
            "scanner",
            title,
            body,
            "0",
            "0",
            "10000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(e) = r {
        warn!("scanners: no notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scanner_is_what_sane_lists_less_webcams_and_the_test_device() {
        let out = "device `airscan:e0:Canon MF240' is a eSCL Canon MF240 ip=192.168.1.40\n\
                   device `v4l:/dev/video0' is a Noname Integrated Camera virtual device\n\
                   device `test:0' is a Noname frontend-tester virtual device\n\
                   device `hpaio:/usb/DeskJet_2700?serial=X' is a Hewlett-Packard DeskJet_2700 all-in-one\n";
        let found = parse_list(out, false);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].device, "airscan:e0:Canon MF240");
        assert_eq!(found[0].what, "eSCL Canon MF240 ip=192.168.1.40");
        assert_eq!(found[1].what, "Hewlett-Packard DeskJet_2700 all-in-one");
        // (For a rig with no scanner: SANE's virtual device counts.)
        assert_eq!(parse_list(out, true).len(), 3);
        // Nothing there: SANE's long apology is no scanner.
        assert!(parse_list("\nNo scanners were identified. If you were expecting…\n", true).is_empty());
    }
}

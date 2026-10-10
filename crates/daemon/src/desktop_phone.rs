//! What a plugged-in Android gives beyond its folder (Max, 2026-10-09: "what
//! else can we do that is useful and a user will like?"): its photos brought
//! to this computer, files dropped on its icon, its internet, and what there
//! is to know about it.
//!
//! All of it over `adb` where the phone lets this computer in (USB
//! debugging) — it is fast, and it does not need the phone to be in file
//! transfer mode — with the phone's mounted folder as the way round for the
//! two that are only files (photos, a drop). Everything here runs OFF the
//! loop: each call is a process, and a first photo import is minutes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tracing::{info, warn};

use crate::tasks::TaskHandle;

/// Where a file sent to a phone lands.
const DOWNLOAD: &str = "/sdcard/Download/";
/// The folders of a phone that hold its photos and videos.
const ALBUMS: [&str; 2] = ["DCIM", "Pictures"];
/// How many files one `adb pull` is asked for.
const BATCH: usize = 40;

/// Run `adb -s <serial> <args>`; what it said, if it ended well.
fn adb(serial: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("adb")
        .args(["-s", serial])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether `adb` has `serial` as a phone that lets this computer in.
pub(crate) fn ready(serial: &str) -> bool {
    Command::new("adb")
        .arg("devices")
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout).lines().any(|l| {
                let mut parts = l.split_whitespace();
                parts.next() == Some(serial) && parts.next() == Some("device")
            })
        })
}

// ── Photos ───────────────────────────────────────────────────────────────

/// One photo or video on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shot {
    pub size: u64,
    /// Under the phone's storage: `DCIM/Camera/IMG_1.jpg`.
    pub path: String,
}

impl Shot {
    /// Where it goes under the phone's folder in Pictures: its album and
    /// name, without the phone's own top folder (`Camera/IMG_1.jpg`).
    pub fn place(&self) -> PathBuf {
        let mut parts = self.path.split('/');
        parts.next();
        parts.collect()
    }
}

/// `find … -exec stat -c "%s|%n"`'s lines (`2048|/sdcard/DCIM/Camera/a.jpg`)
/// → the shots. Hidden files and folders (thumbnails, the phone's bin) are
/// not photos.
pub(crate) fn parse_shots(listing: &str) -> Vec<Shot> {
    listing
        .lines()
        .filter_map(|line| {
            let (size, path) = line.trim().split_once('|')?;
            let path = path.strip_prefix("/sdcard/")?;
            if path.split('/').any(|part| part.starts_with('.')) {
                return None;
            }
            Some(Shot { size: size.parse().ok()?, path: path.to_owned() })
        })
        .collect()
}

/// The shots that are not under `dest` yet (or are there at another size:
/// a copy cut short, a photo edited since).
pub(crate) fn missing<'a>(shots: &'a [Shot], dest: &Path) -> Vec<&'a Shot> {
    shots
        .iter()
        .filter(|s| std::fs::metadata(dest.join(s.place())).map(|m| m.len()).ok() != Some(s.size))
        .collect()
}

/// The owner's Pictures folder (it is named in their language).
fn pictures_dir() -> PathBuf {
    Command::new("xdg-user-dir")
        .arg("PICTURES")
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Pictures"))
}

/// A folder name out of a phone's name.
fn folder_name(name: &str) -> String {
    let tidy: String = name.chars().map(|c| if c == '/' || c.is_control() { ' ' } else { c }).collect();
    let tidy = tidy.trim().trim_start_matches('.').trim();
    if tidy.is_empty() { "Phone".to_owned() } else { tidy.to_owned() }
}

/// Bring the phone's new photos and videos to `Pictures/<its name>/`, album
/// by album: only what is not there yet, so every time after the first is a
/// top-up. What to tell the owner.
pub(crate) fn import_photos(
    name: &str,
    serial: Option<&str>,
    mount: Option<&Path>,
    task: &TaskHandle,
) -> (String, Option<PathBuf>) {
    let dest = pictures_dir().join(folder_name(name));
    let said = import_photos_said(name, serial, mount, task);
    // The folder to show for it, once there is one.
    (said, dest.is_dir().then_some(dest))
}

/// The notification's words. Its TITLE is the phone's name, so they do not
/// repeat it (Max, 2026-10-10: not "Desktop" — "Pixel 8 Pro: 121 photos
/// imported, click to view").
fn import_photos_said(name: &str, serial: Option<&str>, mount: Option<&Path>, task: &TaskHandle) -> String {
    use crate::i18n::tr;
    let dest = pictures_dir().join(folder_name(name));
    let brought = match serial.filter(|s| ready(s)) {
        Some(serial) => import_over_adb(serial, &dest, task),
        None => match mount {
            Some(mount) => import_from_folder(mount, &dest, task),
            None => None,
        },
    };
    // What it says is what the click does (Max: "Pixel 8 Pro: click to view
    // imported photos").
    let view = tr("Click to view imported photos");
    // Cancelled from the bar: what came is kept, and said.
    if task.cancelled() {
        return match brought.map_or(0, |(n, _)| n) {
            0 => tr("Import stopped.").to_owned(),
            _ => format!("{} {view}.", tr("Import stopped.")),
        };
    }
    match brought {
        None => tr("Its photos could not be read. Unlock it and choose File transfer, or turn on USB debugging.").to_owned(),
        Some((0, 0)) => tr("No new photos.").to_owned(),
        Some((_, 0)) => view.to_owned(),
        Some((_, failed)) => format!("{view}. {failed} {}", tr("could not be copied.")),
    }
}

/// How many came, how many did not. `None`: the phone could not be asked.
fn import_over_adb(serial: &str, dest: &Path, task: &TaskHandle) -> Option<(usize, usize)> {
    let albums: Vec<String> = ALBUMS.iter().map(|a| format!("/sdcard/{a}")).collect();
    // (A folder that is not there makes `find` end badly with the rest
    // listed all the same: what it listed is read either way.)
    let listing = Command::new("adb")
        .args(["-s", serial, "shell"])
        .arg(format!("find {} -type f -exec stat -c '%s|%n' {{}} + 2>/dev/null", albums.join(" ")))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let shots = parse_shots(&String::from_utf8_lossy(&listing.stdout));
    let wanted = missing(&shots, dest);
    info!("phone: {} photos and videos on {serial}, {} new", shots.len(), wanted.len());
    // Counted in bytes: a phone's videos are hundreds of times its
    // screenshots, and a bar counted in files would crawl and then jump.
    let total: u64 = wanted.iter().map(|s| s.size).sum();
    let mut done: u64 = 0;
    task.set(0, total);
    // One `adb pull` a folder's batch: it takes many files and one place.
    let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<&Shot>> = Default::default();
    for shot in &wanted {
        let dir = shot.place().parent().map(Path::to_path_buf).unwrap_or_default();
        by_dir.entry(dir).or_default().push(shot);
    }
    for (dir, shots) in &by_dir {
        let into = dest.join(dir);
        if let Err(e) = std::fs::create_dir_all(&into) {
            warn!("phone: cannot make {}: {e}", into.display());
            continue;
        }
        for batch in shots.chunks(BATCH) {
            let mut cmd = Command::new("adb");
            cmd.args(["-s", serial, "pull", "-a"]);
            for shot in batch {
                cmd.arg(format!("/sdcard/{}", shot.path));
            }
            // (The trailing slash: a place for many files, not a name.)
            cmd.arg(format!("{}/", into.display()));
            // While it runs, how much of the batch has landed is read off
            // the files themselves (adb writes each as it comes).
            let landed = || -> u64 {
                batch
                    .iter()
                    .map(|s| std::fs::metadata(dest.join(s.place())).map(|m| m.len().min(s.size)).unwrap_or(0))
                    .sum()
            };
            match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                Ok(mut child) => {
                    while matches!(child.try_wait(), Ok(None)) {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        task.set(done + landed(), total);
                        // Cancelled from the bar: the copy in hand is cut
                        // (its half file is brought whole next time).
                        if task.cancelled() {
                            let _ = child.kill();
                        }
                    }
                }
                Err(e) => warn!("phone: adb could not be run: {e}"),
            }
            done += landed();
            task.set(done, total);
            if task.cancelled() {
                break;
            }
        }
        if task.cancelled() {
            break;
        }
    }
    let left = missing(&shots, dest).len();
    Some((wanted.len() - left.min(wanted.len()), left))
}

/// The same, read from the phone's mounted folder (no `adb`): slower, and
/// the only way for a phone whose owner has not turned debugging on.
fn import_from_folder(mount: &Path, dest: &Path, task: &TaskHandle) -> Option<(usize, usize)> {
    let storages: Vec<PathBuf> = std::fs::read_dir(mount).ok()?.flatten().map(|e| e.path()).collect();
    // What there is to bring, first: the bar needs the whole of it.
    let mut wanted: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    let mut any = false;
    for storage in &storages {
        for album in ALBUMS {
            let mut stack = vec![storage.join(album)];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                any = true;
                for entry in entries.flatten() {
                    let (path, name) = (entry.path(), entry.file_name());
                    if name.to_string_lossy().starts_with('.') {
                        continue;
                    }
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    // `…/DCIM/Camera/a.jpg` → `Camera/a.jpg`.
                    let Ok(place) = path.strip_prefix(storage.join(album)) else {
                        continue;
                    };
                    let to = dest.join(place);
                    let Ok(size) = entry.metadata().map(|m| m.len()) else {
                        continue;
                    };
                    if std::fs::metadata(&to).map(|m| m.len()).ok() != Some(size) {
                        wanted.push((path, to, size));
                    }
                }
            }
        }
    }
    let total: u64 = wanted.iter().map(|w| w.2).sum();
    let (mut came, mut failed, mut done) = (0, 0, 0u64);
    task.set(0, total);
    for (path, to, size) in &wanted {
        if task.cancelled() {
            break;
        }
        let copied = to.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::copy(path, to));
        match copied {
            Ok(_) => came += 1,
            Err(e) => {
                warn!("phone: {} was not copied: {e}", path.display());
                failed += 1;
            }
        }
        done += size;
        task.set(done, total);
    }
    any.then_some((came, failed))
}

// ── Files dropped on its icon ────────────────────────────────────────────

/// Copy `paths` (files, whole folders) to the phone's Download folder.
/// What to tell the owner.
pub(crate) fn push(
    name: &str,
    serial: Option<&str>,
    mount: Option<&Path>,
    paths: &[PathBuf],
    task: &TaskHandle,
) -> String {
    let all = paths.len() as u64;
    task.set(0, all);
    let (came, failed) = match serial.filter(|s| ready(s)) {
        Some(serial) => {
            let (mut came, mut tried) = (0, 0u64);
            // One at a time: the bar moves a file at a time.
            for path in paths {
                if task.cancelled() {
                    break;
                }
                let mut cmd = Command::new("adb");
                cmd.args(["-s", serial, "push"]).arg(path).arg(DOWNLOAD);
                if cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
                {
                    came += 1;
                }
                tried += 1;
                task.set(tried, all);
            }
            (came, paths.len() - came)
        }
        None => match mount {
            Some(mount) => {
                let (copies, failed) = crate::desktop_send::put(paths, &crate::desktop_send::phone_dest(mount), false);
                (copies.len(), failed)
            }
            None => (0, paths.len()),
        },
    };
    let what = |n: usize| if n == 1 { crate::i18n::tr("file") } else { crate::i18n::tr("files") };
    match (came, failed) {
        (0, _) => format!("{name}: {}", crate::i18n::tr("nothing could be copied to it. Unlock it and try again.")),
        (n, 0) => format!("{name}: {n} {} {}", what(n), crate::i18n::tr("copied to its Download folder.")),
        (n, f) => format!(
            "{name}: {n} {} {}; {f} {}",
            what(n),
            crate::i18n::tr("copied to its Download folder"),
            crate::i18n::tr("could not be copied.")
        ),
    }
}

// ── Its internet ─────────────────────────────────────────────────────────

/// Turn the phone's USB tethering on (this computer uses its internet) or
/// off (back to file transfer). Whether the phone took the order; the cable
/// shows whether it did it (`mounts::UsbPhone::tether`).
pub(crate) fn tether(serial: &str, function: &str) -> bool {
    adb(serial, &["shell", "svc", "usb", "setFunctions", function]).is_some()
}

// ── What there is to know about it ───────────────────────────────────────

/// The lines of a phone's Properties box, asked of the phone in one go.
/// Empty when it would not answer.
pub(crate) fn facts(serial: &str) -> Vec<(&'static str, String)> {
    let asked = adb(
        serial,
        &[
            "shell",
            "getprop ro.product.manufacturer; getprop ro.product.model; getprop ro.build.version.release; \
             echo ==; dumpsys battery; echo ==; df -k /sdcard",
        ],
    );
    asked.map(|said| parse_facts(&said)).unwrap_or_default()
}

/// What `facts` asked, read: maker, model and Android version on three
/// lines, `==`, the battery's dump, `==`, `df -k` of its storage.
pub(crate) fn parse_facts(said: &str) -> Vec<(&'static str, String)> {
    let mut parts = said.split("==");
    let mut head = parts.next().unwrap_or("").lines().map(str::trim);
    let (maker, model, android) = (head.next().unwrap_or(""), head.next().unwrap_or(""), head.next().unwrap_or(""));
    let battery = parts.next().unwrap_or("");
    let storage = parts.next().unwrap_or("");
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if !model.is_empty() {
        // (Most models' names start with the maker's already.)
        let named = if maker.is_empty() || model.to_lowercase().starts_with(&maker.to_lowercase()) {
            model.to_owned()
        } else {
            format!("{maker} {model}")
        };
        rows.push(("Model", named));
    }
    if !android.is_empty() {
        rows.push(("Android", android.to_owned()));
    }
    let field = |key: &str| {
        battery.lines().find_map(|l| l.trim().strip_prefix(key)?.trim_start().strip_prefix(':').map(|v| v.trim().to_owned()))
    };
    if let Some(level) = field("level").and_then(|l| l.parse::<u32>().ok()) {
        // 2 charging, 5 full (BatteryManager's own numbers).
        let state = match field("status").as_deref() {
            Some("2") => ", charging",
            Some("5") => ", full",
            _ => "",
        };
        rows.push(("Battery", format!("{level}%{state}")));
    }
    // `Filesystem 1K-blocks Used Available Use% Mounted on`, then its line.
    let numbers: Vec<u64> = storage
        .lines()
        .last()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|n| n.parse::<u64>().ok())
        .collect();
    if let [total, _used, free, ..] = numbers[..] {
        rows.push((
            "Storage",
            format!(
                "{} free of {}",
                crate::desktop_props::size_text(free * 1024),
                crate::desktop_props::size_text(total * 1024)
            ),
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phones_photos_are_listed_and_only_the_new_ones_wanted() {
        let listing = "2048|/sdcard/DCIM/Camera/IMG_1.jpg\n4096|/sdcard/DCIM/Camera/VID_2.mp4\n\
                       10|/sdcard/DCIM/.thumbnails/1.jpg\n77|/sdcard/Pictures/Screenshots/s.png\n\
                       5|/sdcard/Pictures/.trashed-123-x.jpg\nfind: /sdcard/Nope: No such file\n";
        let shots = parse_shots(listing);
        assert_eq!(
            shots.iter().map(|s| (s.size, s.path.as_str())).collect::<Vec<_>>(),
            vec![(2048, "DCIM/Camera/IMG_1.jpg"), (4096, "DCIM/Camera/VID_2.mp4"), (77, "Pictures/Screenshots/s.png")]
        );
        assert_eq!(shots[0].place(), PathBuf::from("Camera/IMG_1.jpg"));
        assert_eq!(shots[2].place(), PathBuf::from("Screenshots/s.png"));

        let dest = std::env::temp_dir().join(format!("wr-photos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(dest.join("Camera")).unwrap();
        // One is there whole, one cut short: only the first is not wanted.
        std::fs::write(dest.join("Camera/IMG_1.jpg"), vec![0u8; 2048]).unwrap();
        std::fs::write(dest.join("Camera/VID_2.mp4"), vec![0u8; 100]).unwrap();
        let wanted: Vec<&str> = missing(&shots, &dest).iter().map(|s| s.path.as_str()).collect();
        let _ = std::fs::remove_dir_all(&dest);
        assert_eq!(wanted, vec!["DCIM/Camera/VID_2.mp4", "Pictures/Screenshots/s.png"]);
        assert_eq!(folder_name("Pixel 8 Pro"), "Pixel 8 Pro");
        assert_eq!(folder_name("../x"), "x");
    }

    #[test]
    fn a_phones_photos_come_from_its_folder_too() {
        let root = std::env::temp_dir().join(format!("wr-mtp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (mount, dest) = (root.join("phone"), root.join("Pictures/Phone"));
        let camera = mount.join("Internal storage/DCIM/Camera");
        std::fs::create_dir_all(&camera).unwrap();
        std::fs::create_dir_all(mount.join("Internal storage/DCIM/.thumbnails")).unwrap();
        std::fs::write(camera.join("a.jpg"), b"aaaa").unwrap();
        std::fs::write(camera.join("b.jpg"), b"bb").unwrap();
        std::fs::write(mount.join("Internal storage/DCIM/.thumbnails/t.jpg"), b"t").unwrap();
        let task = TaskHandle::detached();
        assert_eq!(import_from_folder(&mount, &dest, &task), Some((2, 0)));
        assert_eq!(std::fs::read(dest.join("Camera/a.jpg")).unwrap(), b"aaaa");
        assert!(!dest.join(".thumbnails").exists());
        // Again: nothing new.
        assert_eq!(import_from_folder(&mount, &dest, &task), Some((0, 0)));
        // A phone that shows no storage (locked): not "no new photos".
        assert_eq!(import_from_folder(&root.join("nothing"), &dest, &task), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_phone_says_what_it_is() {
        let said = "samsung\nSM-A115M\n12\n==\nCurrent Battery Service state:\n  AC powered: false\n  USB powered: true\n  \
                    status: 2\n  level: 83\n  scale: 100\n  temperature: 301\n==\n\
                    Filesystem     1K-blocks     Used Available Use% Mounted on\n/dev/fuse       25114556 20091644   5022912  81% /storage/emulated\n";
        assert_eq!(
            parse_facts(said),
            vec![
                ("Model", "samsung SM-A115M".to_owned()),
                ("Android", "12".to_owned()),
                ("Battery", "83%, charging".to_owned()),
                ("Storage", format!(
                    "{} free of {}",
                    crate::desktop_props::size_text(5022912 * 1024),
                    crate::desktop_props::size_text(25114556 * 1024)
                )),
            ]
        );
        // Nothing but its name said: only that.
        assert_eq!(parse_facts("Google\nPixel 8 Pro\n17\n==\n==\n"), vec![("Model", "Google Pixel 8 Pro".to_owned()), ("Android", "17".to_owned())]);
        assert!(parse_facts("").is_empty());
    }
}

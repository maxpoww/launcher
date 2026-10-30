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

use crate::phones::Kind;
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

/// Whether a USB device with this serial number is plugged in.
fn on_the_bus(serial: &str, sysfs: &std::path::Path) -> bool {
    std::fs::read_dir(sysfs).is_ok_and(|devices| {
        devices.flatten().any(|d| {
            std::fs::read_to_string(d.path().join("serial")).is_ok_and(|s| s.trim() == serial)
        })
    })
}

/// Whether `adb` has `serial` as a phone that lets this computer in.
pub(crate) fn ready(serial: &str) -> bool {
    // Over USB only: asked of each phone kept in sync every minute, and most of
    // the time it is not plugged in. The bus says so without starting a program.
    if !on_the_bus(serial, std::path::Path::new("/sys/bus/usb/devices")) {
        return false;
    }
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

// ── Its things, brought here (and taken off it) ──────────────────────────

/// One file of the phone's that is its owner's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub size: u64,
    /// When it last changed on the phone (seconds since 1970).
    pub mtime: i64,
    /// Under the phone's storage: `DCIM/Camera/IMG_1.jpg`.
    pub path: String,
    /// Where it goes here.
    pub to: PathBuf,
}

impl Item {
    /// Its copy here, whole: at its place, or beside it (`free_name`) where
    /// another file had that name.
    fn copy_here(&self) -> Option<PathBuf> {
        copy_at(&self.to, self.size)
    }

    /// Nothing to bring: its copy is here — or its place holds a file of the
    /// owner's that is newer than it (changed here after it came: a photo
    /// turned, a document edited), which is left as it is.
    fn here(&self) -> bool {
        self.copy_here().is_some() || theirs(&self.to, self.mtime)
    }
}

/// A copy, `size` bytes, of a phone's file whose place here is `to`: there,
/// or beside it as `name (2).ext`… — where something else had the name when
/// it came (`arrive`). Nothing is EVER copied over a file here: a size that
/// differs is another file (a name the phone used again after one was
/// deleted, or the owner's own edit), not a copy cut short — copies arrive
/// whole or not at all (`ARRIVING`).
fn copy_at(to: &Path, size: u64) -> Option<PathBuf> {
    let whole = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == size);
    if whole(to) {
        return Some(to.to_path_buf());
    }
    std::fs::symlink_metadata(to).ok()?;
    let (dir, name) = (to.parent()?, to.file_name()?.to_str()?);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .take_while(|p| std::fs::symlink_metadata(p).is_ok())
        .find(|p| whole(p))
}

/// Whether the file at `to` is the owner's own and newer than the phone's
/// (`mtime`): changed here after the copy came (`adb pull -a` keeps the
/// phone's time).
fn theirs(to: &Path, mtime: i64) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(to).is_ok_and(|m| m.is_file() && m.mtime() > mtime)
}

/// The hidden folder a copy lands in before it is put in its place, whole —
/// a copy cut short (a pull killed, a phone gone) never stands where a whole
/// one would, and is thrown away.
const ARRIVING: &str = ".golem-arriving";

/// Put what has come at `got` in its place `to` — or, where something is
/// already there, beside it under a free name (never over it). Where it is.
fn arrive(got: &Path, to: &Path) -> std::io::Result<PathBuf> {
    let dest = match (to.parent(), to.file_name().and_then(|n| n.to_str())) {
        (Some(dir), Some(name)) => crate::bt_files::free_name(dir, name),
        _ => to.to_path_buf(),
    };
    std::fs::rename(got, &dest).map(|()| dest)
}

/// `find … -exec stat -c "%s|%Y|%n"`'s lines
/// (`2048|1760000000|/sdcard/DCIM/Camera/a.jpg`) → the items of `kinds`,
/// each with its place under `folder`. Hidden files and folders
/// (thumbnails, the phone's bin) are nobody's.
pub(crate) fn parse_items(listing: &str, kinds: &[&Kind], folder: &Path) -> Vec<Item> {
    listing
        .lines()
        .filter_map(|line| {
            let (size, rest) = line.trim().split_once('|')?;
            let (mtime, path) = rest.split_once('|')?;
            let path = path.strip_prefix("/sdcard/")?;
            if path.split('/').any(|part| part.starts_with('.')) {
                return None;
            }
            let to = kinds.iter().find_map(|kind| crate::phones::place(kind, path, folder))?;
            Some(Item { size: size.parse().ok()?, mtime: mtime.parse().ok()?, path: path.to_owned(), to })
        })
        .collect()
}

/// A word for the phone's shell, whatever is in it.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// What the phone has of `kinds`. `None`: it could not be asked.
fn list(target: &str, kinds: &[&Kind], folder: &Path) -> Option<Vec<Item>> {
    let dirs: Vec<String> = kinds.iter().flat_map(|k| k.dirs.iter()).map(|d| quoted(&format!("/sdcard/{d}"))).collect();
    if dirs.is_empty() {
        return Some(Vec::new());
    }
    // (A folder that is not there makes `find` end badly with the rest
    // listed all the same: what it listed is read either way.)
    let listing = Command::new("adb")
        .args(["-s", target, "shell"])
        .arg(format!("find {} -type f -exec stat -c '%s|%Y|%n' {{}} + 2>/dev/null", dirs.join(" ")))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // Nothing said and a bad end: the phone is not there.
    if listing.stdout.is_empty() && !ready(target) {
        return None;
    }
    Some(parse_items(&String::from_utf8_lossy(&listing.stdout), kinds, folder))
}

/// How a transfer ended.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Moved {
    /// What was copied here this time.
    pub copied: Vec<PathBuf>,
    /// How many were taken off the phone.
    pub removed: usize,
    /// How many could not be copied.
    pub failed: usize,
}

/// Bring what the phone has of `kinds` to `folder`, only what is not here
/// yet — and, with `remove`, take OFF the phone every file that is here
/// whole (what came now, and what came before): a file leaves the phone only
/// once its copy has been checked. `None`: the phone could not be asked.
/// How long a copy may bring nothing before it is taken for dead.
const STALLED: std::time::Duration = std::time::Duration::from_secs(45);

pub(crate) fn transfer(target: &str, kinds: &[&Kind], folder: &Path, remove: bool, task: &TaskHandle) -> Option<Moved> {
    // One at a time: a Clean, an import and the hourly sync of one phone
    // into one folder at once pulled the same files twice — one copy over
    // another's — and the first's list then took them off the phone.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let items = list(target, kinds, folder)?;
    let wanted: Vec<&Item> = items.iter().filter(|i| !i.here()).collect();
    info!("phone: {} files on {target}, {} new", items.len(), wanted.len());
    // Counted in bytes: a phone's videos are hundreds of times its
    // screenshots, and a bar counted in files would crawl and then jump.
    let total: u64 = wanted.iter().map(|i| i.size).sum();
    let mut done: u64 = 0;
    task.set(0, total);
    // One `adb pull` a folder's batch: it takes many files and one place —
    // the folder's ARRIVING one, from where each goes to its own place
    // whole. (Two of one name in a batch would land on each other there:
    // `DCIM/x.jpg` and `Pictures/x.jpg` both go to Photos.)
    let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<Vec<&Item>>> = Default::default();
    for item in &wanted {
        let batches = by_dir.entry(item.to.parent().map(Path::to_path_buf).unwrap_or_default()).or_default();
        let name = item.to.file_name();
        match batches.last_mut() {
            Some(batch) if batch.len() < BATCH && batch.iter().all(|i| i.to.file_name() != name) => batch.push(item),
            _ => batches.push(vec![item]),
        }
    }
    let mut copied: Vec<PathBuf> = Vec::new();
    'copy: for (into, batches) in &by_dir {
        let arriving = into.join(ARRIVING);
        // (What a copy cut short left there before: it comes again whole.)
        let _ = std::fs::remove_dir_all(&arriving);
        if let Err(e) = std::fs::create_dir_all(&arriving) {
            warn!("phone: cannot make {}: {e}", arriving.display());
            continue;
        }
        for batch in batches {
            let got = |i: &Item| i.to.file_name().map(|n| arriving.join(n));
            let mut cmd = Command::new("adb");
            cmd.args(["-s", target, "pull", "-a"]);
            for item in batch {
                cmd.arg(format!("/sdcard/{}", item.path));
            }
            // (The trailing slash: a place for many files, not a name.)
            cmd.arg(format!("{}/", arriving.display()));
            // While it runs, how much of the batch has landed is read off
            // the files themselves (adb writes each as it comes).
            let landed = || -> u64 {
                batch
                    .iter()
                    .map(|i| got(i).and_then(|g| std::fs::metadata(g).ok()).map_or(0, |m| m.len().min(i.size)))
                    .sum()
            };
            let mut stalled = false;
            match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                Ok(mut child) => {
                    // A copy that stops arriving is cut: over Wi-Fi a pull
                    // hung for good on a phone that had dozed off, and the
                    // sync with it (the Acer and a Samsung, 2026-10-10 — one
                    // video, no byte in six minutes, the task pill frozen).
                    let (mut seen, mut since) = (landed(), std::time::Instant::now());
                    while matches!(child.try_wait(), Ok(None)) {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        let now = landed();
                        if now != seen {
                            (seen, since) = (now, std::time::Instant::now());
                        }
                        task.set(done + now, total);
                        // Cancelled from the bar: the copy in hand is cut
                        // (its half file is thrown away; it comes whole next time).
                        if task.cancelled() {
                            let _ = child.kill();
                        } else if since.elapsed() > STALLED {
                            warn!("phone: nothing has come from {target} for {} s; the copy is cut", STALLED.as_secs());
                            let _ = child.kill();
                            stalled = true;
                        }
                    }
                }
                Err(e) => warn!("phone: adb could not be run: {e}"),
            }
            done += landed();
            task.set(done, total);
            // Each that came whole to its place (beside what is there, never
            // over it); what is half is left to be thrown away.
            for item in batch {
                let Some(got) = got(item).filter(|g| std::fs::metadata(g).is_ok_and(|m| m.len() == item.size)) else {
                    continue;
                };
                match arrive(&got, &item.to) {
                    Ok(dest) => copied.push(dest),
                    Err(e) => warn!("phone: {} could not be put in {}: {e}", got.display(), into.display()),
                }
            }
            // (Stalled: the phone is not answering; what is missing comes
            // at the next sync.)
            if task.cancelled() || stalled {
                let _ = std::fs::remove_dir_all(&arriving);
                break 'copy;
            }
        }
        let _ = std::fs::remove_dir_all(&arriving);
    }
    let failed = if task.cancelled() { 0 } else { wanted.len().saturating_sub(copied.len()) };
    let mut removed = 0;
    if remove && !task.cancelled() {
        // Only what has a WHOLE copy here leaves the phone (not one whose
        // place holds the owner's own edit: that copy is not the phone's file).
        let safe: Vec<&Item> = items.iter().filter(|i| i.copy_here().is_some()).collect();
        for batch in safe.chunks(BATCH) {
            if task.cancelled() {
                break;
            }
            let files: Vec<String> = batch.iter().map(|i| quoted(&format!("/sdcard/{}", i.path))).collect();
            if adb(target, &["shell", &format!("rm -f -- {}", files.join(" "))]).is_some() {
                removed += batch.len();
            }
        }
        info!("phone: {removed} files taken off {target}");
    }
    Some(Moved { copied, removed, failed })
}

/// How much the phone holds of each kind, in bytes (`du`, one call).
pub(crate) fn sizes(target: &str) -> std::collections::HashMap<&'static str, u64> {
    let mut out = std::collections::HashMap::new();
    let mut script = String::new();
    for kind in &crate::phones::KINDS {
        let dirs: Vec<String> = kind.dirs.iter().map(|d| quoted(&format!("/sdcard/{d}"))).collect();
        script.push_str(&format!("echo {} $(du -sk {} 2>/dev/null | cut -f1 | tr '\\n' ' ');", kind.key, dirs.join(" ")));
    }
    let Some(said) = adb(target, &["shell", &script]) else {
        return out;
    };
    for line in said.lines() {
        let mut parts = line.split_whitespace();
        if let Some(kind) = parts.next().and_then(crate::phones::kind) {
            out.insert(kind.key, parts.filter_map(|n| n.parse::<u64>().ok()).sum::<u64>() * 1024);
        }
    }
    out
}

/// The photos a phone's old import left in Pictures go to its folder's
/// Photos (a rename: the same disk), once, so there is one place for them.
pub(crate) fn adopt_pictures(model: &str, folder: &Path) {
    let (old, new) = (pictures_dir().join(crate::phones::folder_name(model)), folder.join("Photos"));
    if old.is_dir() && !new.exists() && std::fs::create_dir_all(folder).is_ok() {
        match std::fs::rename(&old, &new) {
            Ok(()) => info!("phone: {} is now {}", old.display(), new.display()),
            Err(e) => warn!("phone: {} could not be moved to {}: {e}", old.display(), new.display()),
        }
    }
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

/// Bring the phone's new photos and videos to its folder's Photos: only
/// what is not there yet, so every time after the first is a top-up. What to
/// tell the owner, and what a click on that opens.
pub(crate) fn import_photos(
    name: &str,
    serial: Option<&str>,
    mount: Option<&Path>,
    folder: &Path,
    task: &TaskHandle,
) -> (String, Option<String>) {
    use crate::i18n::tr;
    adopt_pictures(name, folder);
    let photos = crate::phones::kind("photos");
    let kinds: Vec<&Kind> = photos.into_iter().collect();
    let brought = match serial.filter(|s| ready(s)) {
        Some(serial) => transfer(serial, &kinds, folder, false, task).map(|m| (m.copied, m.failed)),
        None => match mount {
            Some(mount) => import_from_folder(mount, &folder.join("Photos"), task),
            None => None,
        },
    };
    // What a click on the notification opens: where the photos are (Max,
    // 2026-10-10: no selection — "click opens the dir where all is").
    let dest = folder.join("Photos");
    let show = dest.is_dir().then(|| format!("{}{}", crate::NOTIFY_OPEN, dest.display()));
    // The notification's words. Its TITLE is the phone's name, so they do not
    // repeat it; what they say is what the click does, and how many (Max:
    // "Pixel 8 Pro: click to view [quantity] imported photos").
    let view = |n: usize| match n {
        1 => tr("Click to view 1 imported photo").to_owned(),
        n => format!("{} {n} {}", tr("Click to view"), tr("imported photos")),
    };
    let counts = brought.as_ref().map(|(came, failed)| (came.len(), *failed));
    // Cancelled from the bar: what came is kept, and said.
    let said = if task.cancelled() {
        match counts.map_or(0, |(n, _)| n) {
            0 => tr("Import stopped.").to_owned(),
            n => format!("{} {}.", tr("Import stopped."), view(n)),
        }
    } else {
        match counts {
            None => tr("Its photos could not be read. Unlock it and choose File transfer, or turn on USB debugging.").to_owned(),
            Some((0, 0)) => tr("No new photos.").to_owned(),
            Some((0, failed)) => format!("{failed} {}", tr("photos could not be copied.")),
            Some((n, 0)) => view(n),
            Some((n, failed)) => format!("{}. {failed} {}", view(n), tr("could not be copied.")),
        }
    };
    (said, show)
}

/// The same photos read from the phone's mounted folder (no `adb`): slower,
/// and the only way for a phone whose owner has not turned debugging on.
fn import_from_folder(mount: &Path, dest: &Path, task: &TaskHandle) -> Option<(Vec<PathBuf>, usize)> {
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
                    let Ok(meta) = entry.metadata() else {
                        continue;
                    };
                    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                    let (size, mtime) = (meta.len(), mtime.map_or(i64::MAX, |d| d.as_secs() as i64));
                    // (As `transfer`: here whole, or the owner's own newer
                    // file in its place — never copied over.)
                    if copy_at(&to, size).is_none() && !theirs(&to, mtime) {
                        wanted.push((path, to, size));
                    }
                }
            }
        }
    }
    let total: u64 = wanted.iter().map(|w| w.2).sum();
    let (mut came, mut failed, mut done) = (Vec::new(), 0, 0u64);
    task.set(0, total);
    for (path, to, size) in &wanted {
        if task.cancelled() {
            break;
        }
        let copied = to.parent().ok_or_else(|| std::io::Error::other("no folder")).and_then(|into| {
            let arriving = into.join(ARRIVING);
            std::fs::create_dir_all(&arriving)?;
            let got = arriving.join(to.file_name().unwrap_or_default());
            let made = std::fs::copy(path, &got).and_then(|_| {
                // (The phone's time kept, as `adb pull -a` does: the copy is
                // then known from a later edit of the owner's.)
                let when = std::fs::metadata(path)?.modified()?;
                std::fs::File::options().write(true).open(&got)?.set_modified(when)?;
                arrive(&got, to)
            });
            let _ = std::fs::remove_dir_all(&arriving);
            made
        });
        match copied {
            Ok(dest) => came.push(dest),
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

// ── Over Wi-Fi ───────────────────────────────────────────────────────────

/// The port a phone answers `adb` on over Wi-Fi.
const WIFI_PORT: u16 = 5555;

/// Let a plugged-in phone answer over Wi-Fi from now on (until it restarts:
/// that is the phone's rule): where it will answer (`192.168.1.82:5555`).
/// `None` when it has no Wi-Fi address, or would not.
pub(crate) fn wifi_allow(serial: &str) -> Option<String> {
    let said = adb(serial, &["shell", "ip -f inet addr show wlan0"])?;
    let ip = said.lines().find_map(|l| l.trim().strip_prefix("inet ")?.split('/').next().map(str::to_owned))?;
    adb(serial, &["tcpip", &WIFI_PORT.to_string()])?;
    Some(format!("{ip}:{WIFI_PORT}"))
}

/// Whether something answers at `addr` (a plain knock, a second at most:
/// asked every minute of a phone that is mostly not there).
pub(crate) fn wifi_there(addr: &str) -> bool {
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|a| std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_secs(1)).is_ok())
}

/// The phone over Wi-Fi and nothing else, with ten seconds' patience: it was
/// allowed a moment ago and its own side is still restarting. What `adb -s`
/// is told.
pub(crate) fn reach_wifi(serial: &str, addr: Option<&str>) -> Option<String> {
    let addr = addr?;
    for _ in 0..20 {
        if wifi_there(addr) {
            let _ = Command::new("adb").args(["connect", addr]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            if ready(addr) {
                return is_phone(addr, serial).then(|| addr.to_owned());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    None
}

/// Whether the machine at `addr` is THERE and says no: a phone on the Wi-Fi
/// whose door for us is shut — it was restarted and has forgotten that this
/// computer may come in (a phone that is away does not answer at all).
pub(crate) fn wifi_shut(addr: &str) -> bool {
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs().ok().and_then(|mut a| a.next()).is_some_and(|a| {
        matches!(
            std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_secs(1)),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused
        )
    })
}

/// How to reach the phone now: by its cable if it is on it, else over Wi-Fi
/// if it answers there. What `adb -s` is told.
pub(crate) fn reach(serial: &str, addr: Option<&str>) -> Option<String> {
    // The cable first, and with a little patience: right after Wi-Fi was
    // allowed (`adb tcpip`) the phone's own side restarts and is off its
    // cable for a few seconds — a sync begun then went the slow way round.
    // (While it restarts it is not listed at all, so "is it plugged?" cannot
    // be asked: with a Wi-Fi way to fall back on, the cable is simply given
    // eight seconds to show up. A sync is background work; it can wait.)
    for _ in 0..16 {
        if ready(serial) {
            return Some(serial.to_owned());
        }
        if addr.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let addr = addr?;
    if !wifi_there(addr) {
        return None;
    }
    let _ = Command::new("adb").args(["connect", addr]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    (ready(addr) && is_phone(addr, serial)).then(|| addr.to_owned())
}

/// Whether what answers at `target` is the phone `serial` — an address on
/// the Wi-Fi is the router's to give, and another device that lets this
/// computer in (a second phone, a tablet, a TV) may have this one's now: it
/// would be synced into this phone's folder, and cleaned.
pub(crate) fn is_phone(target: &str, serial: &str) -> bool {
    target == serial || adb(target, &["shell", "getprop ro.serialno"]).is_some_and(|said| said.trim() == serial)
}

/// Whether the phone is on a charger (cable, dock or pad).
pub(crate) fn charging(target: &str) -> bool {
    adb(target, &["shell", "dumpsys battery"])
        .is_some_and(|said| said.lines().any(|l| l.contains("powered: true")))
}

/// How full its battery is, in hundredths (`None`: it would not say).
pub(crate) fn battery(target: &str) -> Option<u8> {
    battery_level(&adb(target, &["shell", "dumpsys battery"])?)
}

/// Its Android, as an API level (31 is Android 12).
pub(crate) fn android_level(target: &str) -> Option<u32> {
    adb(target, &["shell", "getprop ro.build.version.sdk"])?.trim().parse().ok()
}

/// `  level: 66` among what `dumpsys battery` says.
fn battery_level(said: &str) -> Option<u8> {
    said.lines().find_map(|l| l.trim().strip_prefix("level:")?.trim().parse::<u8>().ok()).filter(|l| *l <= 100)
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
    #[test]
    fn a_phone_on_the_bus_is_found_by_its_serial() {
        let bus = std::env::temp_dir().join(format!("phone-bus-{}", std::process::id()));
        let device = bus.join("3-9");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::create_dir_all(bus.join("usb3")).unwrap();
        std::fs::write(device.join("serial"), "3A301FDJG000UW\n").unwrap();
        assert!(super::on_the_bus("3A301FDJG000UW", &bus));
        assert!(!super::on_the_bus("6XTFL18519000092", &bus));
        assert!(!super::on_the_bus("3A301FDJG000UW", &bus.join("nothing")));
        std::fs::remove_dir_all(&bus).unwrap();
    }

    #[test]
    fn the_battery_is_read_from_what_the_phone_says() {
        let said = "Current Battery Service state:\n  AC powered: false\n  USB powered: true\n  status: 2\n  level: 66\n  scale: 100\n  temperature: 201\n";
        assert_eq!(super::battery_level(said), Some(66));
        assert_eq!(super::battery_level("  status: 2\n"), None);
        assert_eq!(super::battery_level("  level: 250\n"), None);
    }

    #[test]
    fn a_shut_door_is_told_from_an_open_one() {
        // A port nobody listens on, on a machine that is there: shut.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        assert!(!super::wifi_shut(&addr), "something answers there");
        assert!(super::wifi_there(&addr));
        drop(listener);
        assert!(super::wifi_shut(&addr), "the machine is there and says no");
        assert!(!super::wifi_there(&addr));
        assert!(!super::wifi_shut("not an address"));
    }

    use super::*;

    #[test]
    fn a_phones_things_are_listed_with_their_places_and_only_the_new_ones_wanted() {
        let listing = "2048|1700000000|/sdcard/DCIM/Camera/IMG_1.jpg\n4096|1700000000|/sdcard/DCIM/Camera/VID_2.mp4\n\
                       10|1700000000|/sdcard/DCIM/.thumbnails/1.jpg\n77|1700000000|/sdcard/Pictures/Screenshots/s.png\n\
                       5|1700000000|/sdcard/Pictures/.trashed-123-x.jpg\n9|1700000000|/sdcard/Music/it's a song.mp3\n\
                       3|1700000000|/sdcard/Alarms/ring.ogg\nfind: /sdcard/Nope: No such file\n";
        let folder = std::env::temp_dir().join(format!("wr-phone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let kinds: Vec<&Kind> = ["photos", "music"].iter().filter_map(|k| crate::phones::kind(k)).collect();
        let items = parse_items(listing, &kinds, &folder);
        assert_eq!(
            items.iter().map(|i| (i.size, i.path.as_str())).collect::<Vec<_>>(),
            vec![
                (2048, "DCIM/Camera/IMG_1.jpg"),
                (4096, "DCIM/Camera/VID_2.mp4"),
                (77, "Pictures/Screenshots/s.png"),
                (9, "Music/it's a song.mp3"),
            ],
            "hidden things and other folders are nobody's"
        );
        assert_eq!(items[0].to, folder.join("Photos/Camera/IMG_1.jpg"));
        assert_eq!(items[3].to, folder.join("Music/it's a song.mp3"));
        assert_eq!(items[0].mtime, 1_700_000_000);
        // One is here whole, one's place holds an older other file: only the
        // first is not wanted, and only the first may leave the phone.
        std::fs::create_dir_all(folder.join("Photos/Camera")).unwrap();
        std::fs::write(&items[0].to, vec![0u8; 2048]).unwrap();
        std::fs::write(&items[1].to, vec![0u8; 100]).unwrap();
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options().write(true).open(&items[1].to).unwrap().set_modified(old).unwrap();
        let here: Vec<&str> = items.iter().filter(|i| i.here()).map(|i| i.path.as_str()).collect();
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(here, vec!["DCIM/Camera/IMG_1.jpg"]);
        // The phone's shell gets each name whole, whatever is in it.
        assert_eq!(quoted("/sdcard/Music/it's a song.mp3"), "'/sdcard/Music/it'\\''s a song.mp3'");
    }

    #[test]
    fn nothing_here_is_ever_copied_over_and_the_owners_edit_stays() {
        let d = std::env::temp_dir().join(format!("wr-arrive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(ARRIVING)).unwrap();
        let to = d.join("document.pdf");
        let phone = |size: u64, mtime: i64| Item { size, mtime, path: "Download/document.pdf".into(), to: to.clone() };
        // The first one came (the phone's time kept, as `adb pull -a` does).
        std::fs::write(&to, b"first version").unwrap();
        let first_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options().write(true).open(&to).unwrap().set_modified(first_time).unwrap();
        assert!(phone(13, 1_700_000_000).here());
        // Deleted on the phone, a new one of the same name downloaded later:
        // wanted — and it arrives BESIDE the first, which stays.
        let newer = phone(6, 1_800_000_000);
        assert!(!newer.here());
        std::fs::write(d.join(ARRIVING).join("document.pdf"), b"second").unwrap();
        let landed = arrive(&d.join(ARRIVING).join("document.pdf"), &to).unwrap();
        assert_eq!(landed, d.join("document (2).pdf"));
        assert_eq!(std::fs::read(&to).unwrap(), b"first version");
        assert!(newer.here(), "found beside its place next time");
        assert_eq!(newer.copy_here(), Some(landed));
        // The owner edits the first copy here (newer than the phone's):
        // nothing is brought over it, and it is no copy to clean the phone by.
        std::fs::write(&to, b"first version, signed").unwrap();
        let first = phone(13, 1_700_000_000);
        assert!(first.here());
        assert_eq!(first.copy_here(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Against a REAL phone (`GOLEM_TEST_PHONE=<serial> cargo test -- --ignored
    /// real_phone`): a scratch folder made on it is brought here, and taken
    /// off it — the file whose copy here is wrong stays on the phone.
    #[test]
    #[ignore]
    fn a_real_phone_is_copied_from_and_cleaned() {
        let Ok(serial) = std::env::var("GOLEM_TEST_PHONE") else {
            return;
        };
        const SCRATCH: Kind = Kind { key: "scratch", label: "Scratch", dirs: &["Download/golem-test"], folder: "Scratch", usual: false };
        let sh = |script: &str| adb(&serial, &["shell", script]).expect("the phone answers");
        sh("rm -rf /sdcard/Download/golem-test; mkdir -p '/sdcard/Download/golem-test/a b'; \
            echo one > /sdcard/Download/golem-test/one.txt; \
            echo two > \"/sdcard/Download/golem-test/a b/it's two.txt\"; \
            echo three > /sdcard/Download/golem-test/three.txt; echo x > /sdcard/Download/golem-test/.hidden");
        let folder = std::env::temp_dir().join(format!("wr-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let task = TaskHandle::detached();
        // Copy only.
        let moved = transfer(&serial, &[&SCRATCH], &folder, false, &task).expect("it lists");
        assert_eq!((moved.copied.len(), moved.removed, moved.failed), (3, 0, 0));
        assert_eq!(std::fs::read_to_string(folder.join("Scratch/a b/it's two.txt")).unwrap(), "two\n");
        assert!(sh("ls /sdcard/Download/golem-test").contains("one.txt"), "a copy takes nothing off the phone");
        // Again: nothing new.
        assert_eq!(transfer(&serial, &[&SCRATCH], &folder, false, &task).unwrap().copied.len(), 0);
        // One copy here was changed by its owner since: it is theirs — not
        // brought again over it, and no copy to take the phone's by.
        std::fs::write(folder.join("Scratch/three.txt"), b"x").unwrap();
        let cleaned = transfer(&serial, &[&SCRATCH], &folder, true, &task).unwrap();
        assert_eq!((cleaned.copied.len(), cleaned.removed, cleaned.failed), (0, 2, 0));
        assert_eq!(std::fs::read_to_string(folder.join("Scratch/three.txt")).unwrap(), "x");
        let mut left: Vec<String> = sh("find /sdcard/Download/golem-test -type f").lines().map(str::to_owned).collect();
        left.sort();
        assert_eq!(
            left,
            ["/sdcard/Download/golem-test/.hidden", "/sdcard/Download/golem-test/three.txt"],
            "the one with no whole copy here stays; the hidden one is not ours"
        );
        sh("rm -rf /sdcard/Download/golem-test");
        let _ = std::fs::remove_dir_all(&folder);
        assert!(sizes(&serial).contains_key("photos"));
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
        let counts = |r: Option<(Vec<PathBuf>, usize)>| r.map(|(came, failed)| (came.len(), failed));
        assert_eq!(counts(import_from_folder(&mount, &dest, &task)), Some((2, 0)));
        assert_eq!(std::fs::read(dest.join("Camera/a.jpg")).unwrap(), b"aaaa");
        assert!(!dest.join(".thumbnails").exists());
        // Again: nothing new.
        assert_eq!(counts(import_from_folder(&mount, &dest, &task)), Some((0, 0)));
        // A phone that shows no storage (locked): not "no new photos".
        assert_eq!(counts(import_from_folder(&root.join("nothing"), &dest, &task)), None);
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

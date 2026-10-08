//! Where a desktop selection can GO, beyond this computer: the "Move to"
//! page of the desktop's menu (`desktop_menu.rs`, `Menu::show_targets`).
//!
//! Two kinds of place. A STICK: a volume mounted under the user's media
//! folder (`/run/media/<user>/…`) — the files are moved onto it (copied,
//! then taken off the desktop once every one of them has arrived). A
//! DEVICE: a phone or another computer paired with this one through KDE
//! Connect (`kdeconnect-cli`, the service Golem already runs for the phone)
//! and reachable right now — the files are SENT to it and stay here; a
//! folder cannot be sent that way and is left alone.
//!
//! Sticks are read at once (`/proc/mounts`); devices take a moment to ask
//! for, so the desktop asks off the loop and the page fills in when the
//! answer comes. Everything that moves or sends runs off the loop too.

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{debug, info, warn};

/// A place a selection can be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// A mounted volume: its folder.
    Stick(PathBuf),
    /// A paired device, by its KDE Connect id.
    Device(String),
}

/// A place and the name it goes by on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub name: String,
    pub place: Place,
    /// Another computer on the network (the page's second list), not
    /// something plugged in or carried.
    pub far: bool,
}

/// The volumes mounted under a media folder in a `/proc/mounts` listing:
/// their folder, and its last part as the name (a volume's label).
pub(crate) fn parse_mounts(mounts: &str) -> Vec<Target> {
    let mut out: Vec<Target> = mounts
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(unescape_mount)
        .filter(|dir| dir.starts_with("/run/media/") || dir.starts_with("/media/"))
        .filter_map(|dir| {
            let path = PathBuf::from(&dir);
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some(Target {
                name,
                place: Place::Stick(path),
                far: false,
            })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.place == b.place);
    out
}

/// `/proc/mounts` writes a space in a path as `\040` (and a tab, a newline
/// and a backslash the same way).
fn unescape_mount(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let code = std::str::from_utf8(&bytes[i + 1..i + 4])
                .ok()
                .and_then(|o| u8::from_str_radix(o, 8).ok());
            if let Some(byte) = code {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where files go on a mounted PHONE (MTP: Android in file-transfer mode).
/// Its top folder only lists its storages and takes no files; inside the
/// first one, Download is where a file sent to a phone is looked for —
/// else that storage itself.
pub(crate) fn phone_dest(mount: &Path) -> PathBuf {
    let mut storages: Vec<PathBuf> = std::fs::read_dir(mount)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    storages.sort();
    let Some(storage) = storages.into_iter().next() else {
        return mount.to_path_buf();
    };
    let download = storage.join("Download");
    if download.is_dir() {
        download
    } else {
        storage
    }
}

/// The sticks plugged in and in use right now.
pub(crate) fn sticks() -> Vec<Target> {
    std::fs::read_to_string("/proc/mounts")
        .map(|m| parse_mounts(&m))
        .unwrap_or_default()
}

/// `kdeconnect-cli -a --id-name-only`: one device a line, its id then its
/// name.
pub(crate) fn parse_devices(listing: &str) -> Vec<(String, String)> {
    listing
        .lines()
        .filter_map(|line| {
            let (id, name) = line.trim().split_once(' ')?;
            let (id, name) = (id.trim(), name.trim());
            // (Its "0 devices found" is a line too.)
            (id.len() >= 16
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.is_empty())
            .then(|| (id.to_owned(), name.to_owned()))
        })
        .collect()
}

/// Whether a KDE Connect device type names a computer (the far list) and
/// not something carried (a phone, a tablet).
pub(crate) fn is_computer(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind.contains("desktop") || kind.contains("laptop")
}

/// The paired devices that can be reached right now. Asks the KDE Connect
/// service, which can take a moment: call it off the loop.
pub(crate) fn devices() -> Vec<Target> {
    let listing = match Command::new("kdeconnect-cli")
        .args(["-a", "--id-name-only"])
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(e) => {
            debug!("desktop: no kdeconnect-cli ({e}); no devices to send to");
            return Vec::new();
        }
    };
    parse_devices(&listing)
        .into_iter()
        .map(|(id, name)| {
            // What it is, from the service's own record of it; not knowing
            // leaves it among the things carried.
            let kind = Command::new("busctl")
                .args([
                    "--user",
                    "get-property",
                    "org.kde.kdeconnect",
                    &format!("/modules/kdeconnect/devices/{id}"),
                    "org.kde.kdeconnect.device",
                    "type",
                ])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            Target {
                name,
                far: is_computer(&kind),
                place: Place::Device(id),
            }
        })
        .collect()
}

/// A name for `name` that is not taken in `dir` (`a (2).txt`…).
pub(crate) fn free_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.split_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Whether `dir` is `src` itself or lies inside it — a copy of `src` into
/// `dir` would then copy into itself, level after level, until the disk is
/// full (a folder holding the desktop dropped ON the desktop).
pub(crate) fn inside(dir: &Path, src: &Path) -> bool {
    match (std::fs::canonicalize(dir), std::fs::canonicalize(src)) {
        (Ok(dir), Ok(src)) => dir.starts_with(src),
        _ => false,
    }
}

/// Copy a file, or a folder with all that is in it, to `dest`. Links are
/// followed, but never round a loop: a folder already entered on this copy
/// (a link back to an ancestor, Wine's `z:` → `/`) is refused.
pub(crate) fn copy_all(src: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.parent().is_some_and(|d| inside(d, src)) {
        return Err(std::io::Error::other("a folder cannot be copied into itself"));
    }
    copy_into(src, dest, &mut Vec::new())
}

fn copy_into(src: &Path, dest: &Path, entered: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if std::fs::metadata(src)?.is_dir() {
        let real = std::fs::canonicalize(src)?;
        if entered.contains(&real) {
            return Err(std::io::Error::other(format!(
                "{} links back into what is being copied",
                src.display()
            )));
        }
        entered.push(real);
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_into(&entry.path(), &dest.join(entry.file_name()), entered)?;
        }
        entered.pop();
        Ok(())
    } else {
        // `fs::copy` also carries the file's permissions over, which a
        // phone's storage (MTP, through gvfs) refuses: there the bytes
        // alone are written.
        if std::fs::copy(src, dest).is_ok() {
            return Ok(());
        }
        let mut from = std::fs::File::open(src)?;
        let mut to = std::fs::File::create(dest)?;
        std::io::copy(&mut from, &mut to).map(|_| ())
    }
}

/// Copy `paths` into `dir` (a name already there gets its number). With
/// `take`, each one is taken away from where it was once it has arrived —
/// a move. The copies' paths, and how many could not be brought.
pub(crate) fn put(paths: &[PathBuf], dir: &Path, take: bool) -> (Vec<PathBuf>, usize) {
    let mut brought = Vec::new();
    let mut failed = 0;
    for src in paths {
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            failed += 1;
            continue;
        };
        // Already there (a cut pasted where it was cut): it is left as it is.
        if src.parent() == Some(dir) {
            brought.push(src.clone());
            continue;
        }
        let dest = free_dest(dir, name);
        // A move within one filesystem is a rename; anything else is a
        // copy, and the original goes only when the copy is whole.
        if take && std::fs::rename(src, &dest).is_ok() {
            brought.push(dest);
            continue;
        }
        match copy_all(src, &dest) {
            Ok(()) => {
                // …and ON the other disk, not only in this one's memory: a
                // stick pulled or a phone that ran out of room must not
                // cost the original.
                let landed = !take
                    || Command::new("sync")
                        .arg("-f")
                        .arg(&dest)
                        .status()
                        .is_ok_and(|s| s.success());
                if take && !landed {
                    warn!(
                        "desktop: {} could not be confirmed on the other side; the original stays",
                        dest.display()
                    );
                }
                if take && landed {
                    let gone = if src.is_dir() {
                        std::fs::remove_dir_all(src)
                    } else {
                        std::fs::remove_file(src)
                    };
                    if let Err(e) = gone {
                        warn!(
                            "desktop: {} arrived but could not be taken away: {e}",
                            src.display()
                        );
                    }
                }
                brought.push(dest);
            }
            Err(e) => {
                warn!(
                    "desktop: cannot put {} in {}: {e}",
                    src.display(),
                    dir.display()
                );
                // Half a copy is worse than none.
                let _ = if dest.is_dir() {
                    std::fs::remove_dir_all(&dest)
                } else {
                    std::fs::remove_file(&dest)
                };
                failed += 1;
            }
        }
    }
    (brought, failed)
}

/// Send `paths` to `place`; what to tell the owner when it is over. Runs
/// for as long as the copy does: call it off the loop.
pub(crate) fn send(paths: &[PathBuf], target: &Target) -> String {
    match &target.place {
        Place::Stick(dir) => {
            let (brought, failed) = put(paths, dir, true);
            info!(
                "desktop: {} moved to {} ({failed} failed)",
                brought.len(),
                dir.display()
            );
            // Not in the stick's cache only: on it, so it can be pulled.
            let _ = Command::new("sync").arg("-f").arg(dir).status();
            outcome(brought.len(), failed, "moved to", &target.name)
        }
        Place::Device(id) => {
            let (mut sent, mut failed) = (0, 0);
            for path in paths {
                if path.is_dir() {
                    warn!(
                        "desktop: {} is a folder; KDE Connect sends files only",
                        path.display()
                    );
                    failed += 1;
                    continue;
                }
                let ok = Command::new("kdeconnect-cli")
                    .arg("--share")
                    .arg(path)
                    .args(["-d", id])
                    .status()
                    .is_ok_and(|s| s.success());
                if ok {
                    sent += 1;
                } else {
                    warn!(
                        "desktop: sending {} to {} failed",
                        path.display(),
                        target.name
                    );
                    failed += 1;
                }
            }
            info!("desktop: {sent} sent to {} ({failed} failed)", target.name);
            outcome(sent, failed, "sent to", &target.name)
        }
    }
}

/// "3 files moved to PHOTOS", "1 file sent to Pixel 8 · 1 could not be".
fn outcome(done: usize, failed: usize, verb: &str, name: &str) -> String {
    let files = |n: usize| format!("{n} {}", if n == 1 { "file" } else { "files" });
    match (done, failed) {
        (0, f) => format!("{} could not be {verb} {name}", files(f)),
        (d, 0) => format!("{} {verb} {name}", files(d)),
        (d, f) => format!("{} {verb} {name} · {f} could not be", files(d)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_never_copied_into_itself_or_round_a_link() {
        let d = std::env::temp_dir().join(format!("waverunner-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home/Desktop")).unwrap();
        std::fs::write(d.join("home/a.txt"), "a").unwrap();
        // The desktop lies inside "home": home cannot be put on it.
        assert!(inside(&d.join("home/Desktop"), &d.join("home")));
        assert!(inside(&d.join("home"), &d.join("home")));
        assert!(!inside(&d.join("home"), &d.join("home/Desktop")));
        assert!(copy_all(&d.join("home"), &d.join("home/Desktop/home")).is_err());
        assert!(!d.join("home/Desktop/home").exists(), "nothing was started");
        // A link back to an ancestor ends the copy instead of spinning.
        std::fs::create_dir_all(d.join("tree/sub")).unwrap();
        std::os::unix::fs::symlink(d.join("tree"), d.join("tree/sub/up")).unwrap();
        assert!(copy_all(&d.join("tree"), &d.join("out")).is_err());
        // An honest tree copies whole; a cut pasted where it was is left be.
        std::fs::create_dir_all(d.join("ok/in")).unwrap();
        std::fs::write(d.join("ok/in/f"), "f").unwrap();
        copy_all(&d.join("ok"), &d.join("ok2")).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("ok2/in/f")).unwrap(), "f");
        let (brought, failed) = put(&[d.join("home/a.txt")], &d.join("home"), true);
        assert_eq!((brought, failed), (vec![d.join("home/a.txt")], 0));
        assert!(!d.join("home/a (2).txt").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_phone_takes_files_in_its_first_storages_download() {
        let d = std::env::temp_dir().join(format!("waverunner-phone-{}", std::process::id()));
        // No storage shown (locked): the mount itself.
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(phone_dest(&d), d);
        std::fs::create_dir_all(d.join("Internal shared storage")).unwrap();
        assert_eq!(phone_dest(&d), d.join("Internal shared storage"));
        std::fs::create_dir_all(d.join("Internal shared storage/Download")).unwrap();
        assert_eq!(phone_dest(&d), d.join("Internal shared storage/Download"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn mounted_sticks_are_found_by_their_folder_under_media() {
        let mounts = "\
/dev/nvme0n1p2 / ext4 rw 0 0
/dev/sdb1 /run/media/max/PHOTOS exfat rw 0 0
/dev/sdc1 /run/media/max/My\\040Stick vfat rw 0 0
tmpfs /run/user/1000 tmpfs rw 0 0
/dev/sdd1 /media/old ext4 rw 0 0
";
        let found = parse_mounts(mounts);
        let names: Vec<&str> = found.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["My Stick", "old", "PHOTOS"]);
        assert_eq!(
            found[0].place,
            Place::Stick(PathBuf::from("/run/media/max/My Stick"))
        );
        assert!(found.iter().all(|t| !t.far));
    }

    #[test]
    fn reachable_devices_are_read_by_id_and_name() {
        let listing =
            "638dd51c723f47c08fe089c1b9b7b6ab Pixel 8 Pro\n_abc1234567890abcd_ max laptop\n";
        assert_eq!(
            parse_devices(listing),
            vec![
                (
                    "638dd51c723f47c08fe089c1b9b7b6ab".to_owned(),
                    "Pixel 8 Pro".to_owned()
                ),
                ("_abc1234567890abcd_".to_owned(), "max laptop".to_owned()),
            ]
        );
        // Its own word for "none" is not a device.
        assert!(parse_devices("0 devices found\n").is_empty());
        assert!(is_computer("s \"laptop\"") && is_computer("s \"desktop\""));
        assert!(!is_computer("s \"smartphone\"") && !is_computer(""));
    }

    #[test]
    fn a_move_takes_the_file_and_a_copy_leaves_it_and_names_never_clash() {
        let root = std::env::temp_dir().join(format!("wr-send-{}", std::process::id()));
        let (from, to) = (root.join("from"), root.join("to"));
        std::fs::create_dir_all(from.join("dir")).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("a.txt"), "a").unwrap();
        std::fs::write(from.join("dir/in.txt"), "in").unwrap();
        std::fs::write(to.join("a.txt"), "old").unwrap();

        // A copy: both stay, the newcomer numbered.
        let (brought, failed) = put(&[from.join("a.txt")], &to, false);
        assert_eq!((brought.clone(), failed), (vec![to.join("a (2).txt")], 0));
        assert!(from.join("a.txt").exists());
        assert_eq!(std::fs::read_to_string(to.join("a.txt")).unwrap(), "old");

        // A move: the file and the folder (with what is in it) leave.
        let (brought, failed) = put(&[from.join("a.txt"), from.join("dir")], &to, true);
        assert_eq!(failed, 0);
        assert_eq!(brought, vec![to.join("a (3).txt"), to.join("dir")]);
        assert!(!from.join("a.txt").exists() && !from.join("dir").exists());
        assert_eq!(
            std::fs::read_to_string(to.join("dir/in.txt")).unwrap(),
            "in"
        );

        // What is not there fails, and leaves nothing behind.
        let (brought, failed) = put(&[from.join("gone.txt")], &to, true);
        assert_eq!((brought.len(), failed), (0, 1));
        assert!(!to.join("gone.txt").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_outcome_says_how_many_and_where() {
        assert_eq!(
            outcome(3, 0, "moved to", "PHOTOS"),
            "3 files moved to PHOTOS"
        );
        assert_eq!(
            outcome(1, 1, "sent to", "Pixel 8"),
            "1 file sent to Pixel 8 · 1 could not be"
        );
        assert_eq!(
            outcome(0, 2, "sent to", "Pixel 8"),
            "2 files could not be sent to Pixel 8"
        );
    }
}

//! THE THREE ZONES (Max, 2026-10-22: "the lock we did to the file manager
//! makes no sense, golem is for every user … we will unlock the file
//! manager … add a confirmation for changes made on specific directories").
//! A file window goes everywhere; what a CHANGE may do depends on where:
//!
//! - YOURS: Home and below (but the delicate places), your sticks, phones
//!   and own disks, `/tmp`, `/var/tmp` — changed freely, Undo covers it.
//! - DELICATE: `~/Golem` (the live system tree), Home's top dot entries
//!   (`.config`, `.ssh`, the browser's profile…), the owner's folders
//!   THEMSELVES (Desktop, Documents…; not what is in them), `~/Phones` — a
//!   change asks first (`confirm.rs`), with one sentence for that place.
//! - THE SYSTEM'S: what the owner cannot write, and everything under
//!   `/etc`, `/nix`, `/usr`, `/var` (but `/var/tmp`), `/run` (but the
//!   owner's media), `/` itself… — looked at, copied from, never changed.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::i18n::tr;

/// Where a change falls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Zone {
    Yours,
    Delicate(Delicate),
    System,
}

/// A delicate place: why it is, and which (`~/Golem`, `~/.config`…).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Delicate {
    pub why: Why,
    pub place: PathBuf,
}

/// Why a place is delicate: the one sentence its confirmation says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// `~/Golem`, the system's own tree.
    Golem,
    /// `.config`, `.local`: the apps' settings.
    Settings,
    /// `.ssh`, `.gnupg`.
    Keys,
    /// The browser's profile (Seam's, a Firefox's).
    Browser,
    /// One of the owner's folders itself (Desktop, Documents…).
    Main,
    /// `~/Phones`.
    Phones,
    /// Any other dot entry of Home.
    Other,
}

impl Why {
    /// What the confirmation says of the place.
    pub fn sentence(self) -> &'static str {
        match self {
            Why::Golem => tr("This folder is your system itself. Changing it can stop the computer from starting."),
            Why::Settings => tr("It holds your apps' settings. Apps can lose them."),
            Why::Keys => tr(
                "It holds your keys for other computers and signatures. Without them you can lose access.",
            ),
            Why::Browser => tr("It holds your browser: tabs, passwords and history."),
            Why::Main => tr("It's one of your main folders. Apps save here by default."),
            Why::Phones => tr("Your phones sync with it. A phone may copy things back or lose them."),
            Why::Other => tr("It's part of how Golem works."),
        }
    }
}

/// The delicate place `path` is, or is in (`home` and `main`, the owner's
/// folders, given): the deepest that says the most — the browser's profile
/// inside `.local`. One of the owner's folders is delicate ITSELF only;
/// what is in it is the owner's like the rest of Home.
pub(crate) fn delicate_at(
    path: &Path,
    home: &Path,
    main: &dyn Fn(&Path) -> bool,
) -> Option<Delicate> {
    let rel = path.strip_prefix(home).ok()?;
    let top = rel
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    let place = |p: PathBuf, why| Some(Delicate { why, place: p });
    match top.as_str() {
        "Golem" => place(home.join("Golem"), Why::Golem),
        "Phones" => place(home.join("Phones"), Why::Phones),
        ".ssh" | ".gnupg" => place(home.join(&top), Why::Keys),
        ".mozilla" => place(home.join(&top), Why::Browser),
        ".local" if rel.starts_with(".local/share/seam") => {
            place(home.join(".local/share/seam"), Why::Browser)
        }
        ".config" | ".local" => place(home.join(&top), Why::Settings),
        _ if top.starts_with('.') => place(home.join(&top), Why::Other),
        _ if main(path) => place(path.to_path_buf(), Why::Main),
        _ => None,
    }
}

/// Whether `abs` lies in the system's own folders by their names: `/`
/// itself, `/etc`, `/nix`, `/usr`…, `/var` but `/var/tmp`, `/run` but the
/// owner's media (`/run/media/<user>/<it>`, gvfs's mounts).
pub(crate) fn system_tree(abs: &Path, user: &str, uid: u32) -> bool {
    let parts: Vec<String> = abs
        .components()
        .skip(1)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let Some(top) = parts.first() else {
        return true; // `/`
    };
    let rest = &parts[1..];
    match top.as_str() {
        "etc" | "nix" | "boot" | "usr" | "proc" | "sys" | "dev" | "root" | "srv" | "opt"
        | "bin" | "sbin" | "lib" | "lib64" => true,
        "var" => rest.first().map(String::as_str) != Some("tmp"),
        "run" => {
            let media = rest.len() >= 3 && rest[0] == "media" && rest[1] == user;
            let gvfs = rest.len() >= 4
                && rest[0] == "user"
                && rest[1] == uid.to_string()
                && rest[2] == "gvfs";
            !(media || gvfs)
        }
        _ => false,
    }
}

/// Where a change TO `path` falls (renaming it, moving it, throwing it
/// away, turning it): its folder must be the owner's to write, and it the
/// owner's.
pub(crate) fn zone_of(path: &Path) -> Zone {
    remembered(path, false)
}

/// Where a change INSIDE folder `dir` falls (something pasted, dropped or
/// made in it): it must be the owner's to write. Inside one of the owner's
/// folders is the owner's, like the rest of Home.
pub(crate) fn zone_in(dir: &Path) -> Zone {
    remembered(dir, true)
}

/// Whether `path` is the system's (a change to it, or in it, is never
/// made): for the bin's hard rule (`trash::protected`).
pub(crate) fn is_system(path: &Path) -> bool {
    zone_now(path, false) == Zone::System
}

/// Remembered a few seconds: it is asked at every frame of a file window
/// with a selection (its pill), for each thing.
fn remembered(path: &Path, inside: bool) -> Zone {
    type Seen = HashMap<(PathBuf, bool), (Zone, Instant)>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    const KEEP: Duration = Duration::from_secs(3);
    let lock = || SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let key = (path.to_path_buf(), inside);
    if let Some((zone, at)) = lock().as_ref().and_then(|seen| seen.get(&key)) {
        if at.elapsed() < KEEP {
            return zone.clone();
        }
    }
    let now = zone_now(path, inside);
    let mut seen = lock();
    let seen = seen.get_or_insert_with(HashMap::new);
    if seen.len() > 4096 {
        seen.clear();
    }
    seen.insert(key, (now.clone(), Instant::now()));
    now
}

fn zone_now(path: &Path, inside: bool) -> Zone {
    // Its folder made real, its own name kept (a link is judged as itself).
    let abs = match (
        path.parent().filter(|p| !p.as_os_str().is_empty()),
        path.file_name(),
    ) {
        (Some(dir), Some(name)) => std::fs::canonicalize(dir).map(|d| d.join(name)),
        _ => std::fs::canonicalize(path),
    }
    .unwrap_or_else(|_| path.to_path_buf());
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let user = std::env::var("USER").unwrap_or_default();
    if system_tree(&abs, &user, uid) {
        return Zone::System;
    }
    // On a slow place (a phone, a camera) the disk is not asked: it is the
    // owner's, plugged in.
    if crate::filer::slow_mount(&abs).is_some() {
        return Zone::Yours;
    }
    let folder = if inside {
        abs.as_path()
    } else {
        abs.parent().unwrap_or(&abs)
    };
    if folder.exists() && !writable(folder) {
        return Zone::System;
    }
    if !inside {
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if std::fs::symlink_metadata(&abs).is_ok_and(|m| m.uid() != me) {
            return Zone::System;
        }
    }
    let home = std::fs::canonicalize(crate::filer::home()).unwrap_or_else(|_| crate::filer::home());
    let main = |p: &Path| p != home && p != home.join("Phones") && crate::desktop::is_own_folder(p);
    match delicate_at(&abs, &home, &main) {
        // (In one of the owner's folders: the owner's.)
        Some(d) if !(inside && d.why == Why::Main) => Zone::Delicate(d),
        _ => Zone::Yours,
    }
}

/// Whether the owner may write in `path`.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Whether `dir` is emptied when the computer starts again (`/tmp`,
/// `/var/tmp`): its chip says so.
pub(crate) fn emptied_at_restart(dir: &Path) -> bool {
    dir.starts_with("/tmp") || dir.starts_with("/var/tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_systems_folders_are_known_by_their_names() {
        let sys = |p: &str| system_tree(Path::new(p), "max", 1000);
        for p in [
            "/",
            "/etc",
            "/etc/hosts",
            "/nix/store/x",
            "/usr/bin",
            "/var",
            "/var/lib/x",
            "/run",
            "/run/media/max",
            "/run/user/1000",
            "/boot",
            "/root",
        ] {
            assert!(sys(p), "{p} is the system's");
        }
        for p in [
            "/tmp",
            "/tmp/a",
            "/var/tmp",
            "/var/tmp/b",
            "/run/media/max/STICK",
            "/run/media/max/STICK/photos",
            "/run/user/1000/gvfs/mtp:host=Pixel",
            "/home/max",
            "/mnt/data",
        ] {
            assert!(!sys(p), "{p} is not the system's by name");
        }
        // Another user's media: the system's.
        assert!(sys("/run/media/ana/STICK"));
    }

    #[test]
    fn the_delicate_places_and_what_they_say() {
        let home = Path::new("/home/max");
        let main = |p: &Path| {
            p == Path::new("/home/max/Documents") || p == Path::new("/home/max/Pictures")
        };
        let why = |p: &str| delicate_at(Path::new(p), home, &main).map(|d| (d.why, d.place));
        assert_eq!(
            why("/home/max/Golem/flake.nix"),
            Some((Why::Golem, home.join("Golem")))
        );
        assert_eq!(
            why("/home/max/Golem"),
            Some((Why::Golem, home.join("Golem")))
        );
        assert_eq!(
            why("/home/max/.config/hypr"),
            Some((Why::Settings, home.join(".config")))
        );
        assert_eq!(
            why("/home/max/.local"),
            Some((Why::Settings, home.join(".local")))
        );
        assert_eq!(
            why("/home/max/.local/share/seam/prefs.js"),
            Some((Why::Browser, home.join(".local/share/seam")))
        );
        assert_eq!(
            why("/home/max/.mozilla"),
            Some((Why::Browser, home.join(".mozilla")))
        );
        assert_eq!(
            why("/home/max/.ssh/id_ed25519"),
            Some((Why::Keys, home.join(".ssh")))
        );
        assert_eq!(
            why("/home/max/.gnupg"),
            Some((Why::Keys, home.join(".gnupg")))
        );
        assert_eq!(
            why("/home/max/.cache/x"),
            Some((Why::Other, home.join(".cache")))
        );
        assert_eq!(
            why("/home/max/Phones/Pixel"),
            Some((Why::Phones, home.join("Phones")))
        );
        // The owner's folders themselves, not what is in them.
        assert_eq!(
            why("/home/max/Pictures"),
            Some((Why::Main, home.join("Pictures")))
        );
        assert_eq!(why("/home/max/Pictures/cat.png"), None);
        assert_eq!(why("/home/max/notes.txt"), None);
        assert_eq!(why("/home/max"), None);
        assert_eq!(why("/tmp/.hidden"), None);
    }

    #[test]
    fn tmp_is_emptied_at_restart() {
        assert!(emptied_at_restart(Path::new("/tmp")));
        assert!(emptied_at_restart(Path::new("/var/tmp/x")));
        assert!(!emptied_at_restart(Path::new("/home/max/tmp")));
    }
}
